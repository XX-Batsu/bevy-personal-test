//! Bridge API 共用 helper：錯誤建構與輸入驗證。
//!
//! 由 `bridge_api.rs`（Phase A）與 `camera_module.rs`（Phase B）共用。
//! 所有 helper 為 `pub(crate)` — 不對外公開，限 vm_runtime crate 內部使用。

use rhai::{EvalAltResult, Position};

/// OpsTracker deduct 錯誤 → Box<EvalAltResult>。
pub(crate) fn ops_err(e: String) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(e.into(), Position::NONE).into()
}

/// 驗證有限數值（reject 策略：NaN/Inf 檢查）。
pub(crate) fn validate_finite(value: f64, param_name: &str) -> Result<(), Box<EvalAltResult>> {
    if !value.is_finite() {
        return Err(Box::new(EvalAltResult::ErrorRuntime(
            format!(
                "InvalidParameter: {} 必須為有限數值，收到 {}",
                param_name, value
            )
            .into(),
            Position::NONE,
        )));
    }
    Ok(())
}

/// 驗證非負 ID（reject 策略：>= 0，適用 dialog_id / anim_id）。
pub(crate) fn validate_non_negative_id(
    value: i64,
    param_name: &str,
) -> Result<(), Box<EvalAltResult>> {
    if value < 0 {
        return Err(Box::new(EvalAltResult::ErrorRuntime(
            format!("InvalidParameter: {} 必須 >= 0，收到 {}", param_name, value).into(),
            Position::NONE,
        )));
    }
    Ok(())
}

/// 驗證正 ID（reject 策略：> 0，適用 sound_id / vfx_id / duration_ms）。
pub(crate) fn validate_positive_id(value: i64, param_name: &str) -> Result<(), Box<EvalAltResult>> {
    if value <= 0 {
        return Err(Box::new(EvalAltResult::ErrorRuntime(
            format!("InvalidParameter: {} 必須 > 0，收到 {}", param_name, value).into(),
            Position::NONE,
        )));
    }
    Ok(())
}

/// 驗證 camera handle 範圍（0 ≤ value ≤ u32::MAX）並回傳 u32。
///
/// 取代 `camera_module::check_handle_range`，命名對齊 `validate_*` 慣例；
/// 多了 `param_name` 參數讓 error message 更精準（其他未來 bridge —
/// audio handle / vfx handle — 也會用相同 i64→u32 saturating 轉換）。
pub(crate) fn validate_handle(value: i64, param_name: &str) -> Result<u32, Box<EvalAltResult>> {
    if !(0..=(u32::MAX as i64)).contains(&value) {
        return Err(Box::new(EvalAltResult::ErrorRuntime(
            format!(
                "InvalidParameter: {} 必須在 [0, u32::MAX]，收到 {}",
                param_name, value
            )
            .into(),
            Position::NONE,
        )));
    }
    Ok(value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_handle_accepts_zero() {
        assert_eq!(validate_handle(0, "handle").unwrap(), 0);
    }

    #[test]
    fn validate_handle_accepts_u32_max() {
        assert_eq!(
            validate_handle(u32::MAX as i64, "handle").unwrap(),
            u32::MAX
        );
    }

    #[test]
    fn validate_handle_rejects_negative() {
        let err = validate_handle(-1, "handle").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("handle"), "error 應包含 param_name: {msg}");
        assert!(msg.contains("-1"), "error 應包含實際值: {msg}");
    }

    #[test]
    fn validate_handle_rejects_above_u32_max() {
        let err = validate_handle(u32::MAX as i64 + 1, "handle").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("handle"), "error 應包含 param_name: {msg}");
    }

    #[test]
    fn validate_finite_accepts_finite() {
        assert!(validate_finite(0.0, "x").is_ok());
        assert!(validate_finite(42.5, "x").is_ok());
        assert!(validate_finite(-1.0, "x").is_ok());
    }

    #[test]
    fn validate_finite_rejects_nan_and_inf() {
        assert!(validate_finite(f64::NAN, "trauma").is_err());
        assert!(validate_finite(f64::INFINITY, "trauma").is_err());
        assert!(validate_finite(f64::NEG_INFINITY, "trauma").is_err());
    }

    #[test]
    fn validate_non_negative_id_accepts_zero_and_positive() {
        assert!(validate_non_negative_id(0, "id").is_ok());
        assert!(validate_non_negative_id(1, "id").is_ok());
    }

    #[test]
    fn validate_non_negative_id_rejects_negative() {
        assert!(validate_non_negative_id(-1, "id").is_err());
    }

    #[test]
    fn validate_positive_id_rejects_zero_and_negative() {
        assert!(validate_positive_id(0, "id").is_err());
        assert!(validate_positive_id(-1, "id").is_err());
    }

    #[test]
    fn validate_positive_id_accepts_positive() {
        assert!(validate_positive_id(1, "id").is_ok());
    }
}
