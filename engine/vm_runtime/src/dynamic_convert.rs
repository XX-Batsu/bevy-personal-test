//! DeterministicValue ↔ rhai::Dynamic 轉換
//!
//! 提供 game logic 確定性值與 Rhai 腳本動態值之間的雙向轉換。
//! 轉換層位於 `vm_runtime`（而非 `bridge_types`），因為 `bridge_types` 不依賴 `rhai`。
//!
//! # Type tag 對應（系統契約）
//! - 0=Int(i64), 1=Float(SoftF32), 2=Bool, 3=Unit, 4=Str(String)
//!
//! # 安全性
//! - `clone_cast::<T>()` 僅在 `is::<T>()` 確認後呼叫，不會 panic
//! - String 轉換透過 `validated_str()` 驗證 4096 bytes 上限
//! - 不支援的型別（Array/Map 等）降級為 Unit 並記錄 warn

use bridge_types::{BridgeError, DeterministicValue};
use deterministic::SoftF32;

/// 將 rhai::Dynamic 轉換為 DeterministicValue
///
/// 轉換規則（依 type tag 順序）：
/// - i64 → DeterministicValue::Int       (tag 0)
/// - f64 → DeterministicValue::Float     (tag 1, 透過 SoftF32::from_f64 轉換)
/// - bool → DeterministicValue::Bool     (tag 2)
/// - () → DeterministicValue::Unit       (tag 3)
/// - String → DeterministicValue::Str    (tag 4, 透過 validated_str() 驗證長度)
///   - 長度 ≤ 4096 bytes → Str(s) 成功轉換
///   - 長度 > 4096 bytes → tracing::warn! + 降級為 Unit
/// - 其他型別（Array/Map/自定義等） → tracing::warn! + 降級為 Unit
///
/// # clone_cast 安全性
/// `rhai::Dynamic::clone_cast::<T>()` 在 `is::<T>()` 為 true 後呼叫，
/// 型別已確認匹配，不會觸發 CastError panic。
pub fn to_deterministic(value: &rhai::Dynamic) -> DeterministicValue {
    if value.is::<i64>() {
        DeterministicValue::Int(value.clone_cast::<i64>())
    } else if value.is::<f64>() {
        let f = value.clone_cast::<f64>();
        DeterministicValue::Float(SoftF32::from_f64(f))
    } else if value.is::<bool>() {
        DeterministicValue::Bool(value.clone_cast::<bool>())
    } else if value.is_unit() {
        DeterministicValue::Unit
    } else if value.is::<rhai::ImmutableString>() {
        let s = value.clone_cast::<rhai::ImmutableString>().to_string();
        match DeterministicValue::validated_str(s) {
            Ok(det) => det,
            Err(BridgeError::StringTooLong { len, max }) => {
                tracing::warn!(
                    len,
                    max,
                    "字串長度超過 DeterministicValue 上限，降級為 Unit"
                );
                DeterministicValue::Unit
            }
            Err(_) => unreachable!("validated_str 僅回傳 StringTooLong"),
        }
    } else {
        // Array、Map 等不支援的型別一律降級為 Unit
        tracing::warn!(
            type_name = value.type_name(),
            "無法轉換的 Dynamic 型別，降級為 Unit"
        );
        DeterministicValue::Unit
    }
}

/// 將 DeterministicValue 轉換為 rhai::Dynamic
///
/// 轉換規則（依 type tag 順序）：
/// - Int(i) → rhai::Dynamic::from(i)              (tag 0)
/// - Float(sf) → rhai::Dynamic::from(sf.to_f64()) (tag 1)
/// - Bool(b) → rhai::Dynamic::from(b)             (tag 2)
/// - Unit → rhai::Dynamic::UNIT                   (tag 3)
/// - Str(s) → rhai::Dynamic::from(s.clone())      (tag 4)
///
/// # 注意
/// Float → f64 轉換使用 `SoftF32::to_f64()`，此值僅供腳本內部計算或渲染輸出使用，
/// 不可在 game logic 中對此 f64 做算術再轉回 SoftF32（會破壞確定性）。
pub fn to_rhai_dynamic(value: &DeterministicValue) -> rhai::Dynamic {
    match value {
        DeterministicValue::Int(i) => rhai::Dynamic::from(*i),
        DeterministicValue::Float(sf) => rhai::Dynamic::from(sf.to_f64()),
        DeterministicValue::Bool(b) => rhai::Dynamic::from(*b),
        DeterministicValue::Unit => rhai::Dynamic::UNIT,
        DeterministicValue::Str(s) => rhai::Dynamic::from(s.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::{DeterministicValue, DETERMINISTIC_STR_MAX_BYTES};
    use std::collections::BTreeMap;

    // ── DeterministicValue bincode round-trip 測試 ──

    #[test]
    fn test_deterministic_int_round_trip() {
        let val = DeterministicValue::Int(42);
        let bytes = bincode::serialize(&val).unwrap();
        let decoded: DeterministicValue = bincode::deserialize(&bytes).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn test_deterministic_float_round_trip() {
        let val = DeterministicValue::Float(SoftF32::from_f64(2.78));
        let bytes = bincode::serialize(&val).unwrap();
        let decoded: DeterministicValue = bincode::deserialize(&bytes).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn test_deterministic_bool_round_trip() {
        let val = DeterministicValue::Bool(true);
        let bytes = bincode::serialize(&val).unwrap();
        let decoded: DeterministicValue = bincode::deserialize(&bytes).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn test_deterministic_unit_round_trip() {
        let val = DeterministicValue::Unit;
        let bytes = bincode::serialize(&val).unwrap();
        let decoded: DeterministicValue = bincode::deserialize(&bytes).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn test_deterministic_str_round_trip() {
        let val = DeterministicValue::Str("hello".to_string());
        let bytes = bincode::serialize(&val).unwrap();
        let decoded: DeterministicValue = bincode::deserialize(&bytes).unwrap();
        assert_eq!(val, decoded);
    }

    // ── rhai::Dynamic → DeterministicValue 轉換測試 ──

    #[test]
    fn test_dynamic_to_deterministic_int() {
        let dyn_val = rhai::Dynamic::from(42_i64);
        let det_val = to_deterministic(&dyn_val);
        assert_eq!(det_val, DeterministicValue::Int(42));
    }

    #[test]
    fn test_dynamic_to_deterministic_float() {
        let dyn_val = rhai::Dynamic::from(2.78_f64);
        let det_val = to_deterministic(&dyn_val);
        // f64 → SoftF32 精度截斷，驗證 bits 一致
        let expected = DeterministicValue::Float(SoftF32::from_f64(2.78));
        assert_eq!(det_val, expected);
    }

    #[test]
    fn test_dynamic_to_deterministic_bool() {
        let dyn_val = rhai::Dynamic::from(true);
        let det_val = to_deterministic(&dyn_val);
        assert_eq!(det_val, DeterministicValue::Bool(true));
    }

    #[test]
    fn test_dynamic_to_deterministic_unit() {
        let dyn_val = rhai::Dynamic::UNIT;
        let det_val = to_deterministic(&dyn_val);
        assert_eq!(det_val, DeterministicValue::Unit);
    }

    #[test]
    fn test_dynamic_to_deterministic_string_ok() {
        // String ≤ 4096 bytes → Str variant（非降級）
        let dyn_val = rhai::Dynamic::from("hello".to_string());
        let det_val = to_deterministic(&dyn_val);
        assert_eq!(det_val, DeterministicValue::Str("hello".to_string()));
    }

    #[test]
    fn test_dynamic_to_deterministic_string_empty() {
        let dyn_val = rhai::Dynamic::from("".to_string());
        let det_val = to_deterministic(&dyn_val);
        assert_eq!(det_val, DeterministicValue::Str("".to_string()));
    }

    #[test]
    fn test_dynamic_to_deterministic_string_at_limit() {
        let s = "a".repeat(DETERMINISTIC_STR_MAX_BYTES); // 剛好 4096 bytes
        let dyn_val = rhai::Dynamic::from(s.clone());
        let det_val = to_deterministic(&dyn_val);
        assert_eq!(det_val, DeterministicValue::Str(s));
    }

    #[test]
    fn test_dynamic_to_deterministic_string_over_limit() {
        // String > 4096 bytes → validated_str() 失敗 → 降級為 Unit
        let s = "a".repeat(DETERMINISTIC_STR_MAX_BYTES + 1);
        let dyn_val = rhai::Dynamic::from(s);
        let det_val = to_deterministic(&dyn_val);
        assert_eq!(det_val, DeterministicValue::Unit);
    }

    #[test]
    fn test_dynamic_to_deterministic_array_fallback() {
        // Array 不在 DeterministicValue 支援清單，降級為 Unit
        let dyn_val = rhai::Dynamic::from(rhai::Array::new());
        let det_val = to_deterministic(&dyn_val);
        assert_eq!(det_val, DeterministicValue::Unit);
    }

    // ── DeterministicValue → rhai::Dynamic 轉換測試 ──

    #[test]
    fn test_deterministic_to_dynamic_int() {
        let det_val = DeterministicValue::Int(42);
        let dyn_val = to_rhai_dynamic(&det_val);
        assert_eq!(dyn_val.clone_cast::<i64>(), 42);
    }

    #[test]
    fn test_deterministic_to_dynamic_float() {
        let sf = SoftF32::from_f64(2.78);
        let det_val = DeterministicValue::Float(sf);
        let dyn_val = to_rhai_dynamic(&det_val);
        let f = dyn_val.clone_cast::<f64>();
        // SoftF32 → f64 的結果應與原始 SoftF32::to_f64() 一致
        assert!((f - sf.to_f64()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_deterministic_to_dynamic_bool() {
        let det_val = DeterministicValue::Bool(false);
        let dyn_val = to_rhai_dynamic(&det_val);
        assert!(!dyn_val.clone_cast::<bool>());
    }

    #[test]
    fn test_deterministic_to_dynamic_unit() {
        let det_val = DeterministicValue::Unit;
        let dyn_val = to_rhai_dynamic(&det_val);
        assert!(dyn_val.is_unit());
    }

    #[test]
    fn test_deterministic_to_dynamic_str() {
        let det_val = DeterministicValue::Str("hello".to_string());
        let dyn_val = to_rhai_dynamic(&det_val);
        assert_eq!(
            dyn_val.clone_cast::<rhai::ImmutableString>().as_str(),
            "hello"
        );
    }

    // ── MirroredEntity.custom BTreeMap 測試 ──

    #[test]
    fn test_mirrored_entity_custom() {
        let mut custom: BTreeMap<String, DeterministicValue> = BTreeMap::new();
        custom.insert("hp".to_string(), DeterministicValue::Int(100));
        custom.insert("mana".to_string(), DeterministicValue::Int(50));
        custom.insert("alive".to_string(), DeterministicValue::Bool(true));

        assert_eq!(custom.get("hp"), Some(&DeterministicValue::Int(100)));
        assert_eq!(custom.get("mana"), Some(&DeterministicValue::Int(50)));
        assert_eq!(custom.get("alive"), Some(&DeterministicValue::Bool(true)));
    }

    #[test]
    fn test_mirrored_entity_custom_iteration_order() {
        let mut custom: BTreeMap<String, DeterministicValue> = BTreeMap::new();
        custom.insert("z_key".to_string(), DeterministicValue::Int(3));
        custom.insert("a_key".to_string(), DeterministicValue::Int(1));
        custom.insert("m_key".to_string(), DeterministicValue::Int(2));

        // BTreeMap 保證字典序遍歷
        let keys: Vec<&String> = custom.keys().collect();
        assert_eq!(keys, vec!["a_key", "m_key", "z_key"]);
    }

    // ── 精度確定性測試 ──

    #[test]
    fn test_float_precision_deterministic() {
        // f64 → SoftF32 截斷應與 f32 → SoftF32 結果一致
        let from_f64 = SoftF32::from_f64(1.0000001_f64);
        let from_f32 = SoftF32::from_f32(1.0000001_f32);
        assert_eq!(from_f64.to_bits(), from_f32.to_bits());
    }

    // ── round-trip 轉換測試（Dynamic → Deterministic → Dynamic） ──

    #[test]
    fn test_int_round_trip_via_dynamic() {
        let original = rhai::Dynamic::from(99_i64);
        let det = to_deterministic(&original);
        let back = to_rhai_dynamic(&det);
        assert_eq!(back.clone_cast::<i64>(), 99);
    }

    #[test]
    fn test_bool_round_trip_via_dynamic() {
        let original = rhai::Dynamic::from(true);
        let det = to_deterministic(&original);
        let back = to_rhai_dynamic(&det);
        assert!(back.clone_cast::<bool>());
    }

    #[test]
    fn test_str_round_trip_via_dynamic() {
        let original = rhai::Dynamic::from("round-trip".to_string());
        let det = to_deterministic(&original);
        let back = to_rhai_dynamic(&det);
        assert_eq!(
            back.clone_cast::<rhai::ImmutableString>().as_str(),
            "round-trip"
        );
    }

    #[test]
    fn test_unit_round_trip_via_dynamic() {
        let original = rhai::Dynamic::UNIT;
        let det = to_deterministic(&original);
        let back = to_rhai_dynamic(&det);
        assert!(back.is_unit());
    }
}
