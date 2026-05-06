//! Bridge 動態值型別
//!
//! `DynamicValue` 用於 script <-> engine 介面中的渲染層場景。
//! 允許 f64：因為此型別用於渲染輸出，不參與確定性模擬。
//!
//! 使用場景：
//! - `BridgeEvent::UpdateHud { value: DynamicValue }`
//! - `BridgeEvent::SendPrediction { data: DynamicValue }`

use serde::{Deserialize, Serialize};

/// Bridge 動態值 — script <-> engine 介面（含渲染層資料）
///
/// 允許 f64：因為此型別用於渲染輸出，不參與確定性模擬。
/// 不可用於 game logic 或 State Hash 計算。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DynamicValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Unit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_value_bincode_round_trip() {
        let values = vec![
            DynamicValue::Int(42),
            DynamicValue::Float(std::f64::consts::PI),
            DynamicValue::Bool(true),
            DynamicValue::String("hello".to_string()),
            DynamicValue::Unit,
        ];
        for val in values {
            let bytes = bincode::serialize(&val).unwrap();
            let decoded: DynamicValue = bincode::deserialize(&bytes).unwrap();
            assert_eq!(val, decoded);
        }
    }

    #[test]
    fn dynamic_value_empty_string() {
        let val = DynamicValue::String(String::new());
        let bytes = bincode::serialize(&val).unwrap();
        let decoded: DynamicValue = bincode::deserialize(&bytes).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn dynamic_value_negative_int() {
        let val = DynamicValue::Int(-9999);
        let bytes = bincode::serialize(&val).unwrap();
        let decoded: DynamicValue = bincode::deserialize(&bytes).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn dynamic_value_float_special() {
        let val = DynamicValue::Float(f64::INFINITY);
        let bytes = bincode::serialize(&val).unwrap();
        let decoded: DynamicValue = bincode::deserialize(&bytes).unwrap();
        assert_eq!(val, decoded);
    }
}
