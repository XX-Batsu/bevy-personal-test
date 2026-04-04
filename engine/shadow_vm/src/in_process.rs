//! InProcessShadowVm — Native 同步 Shadow VM 測試替身
//!
//! 提供不依賴 Web Worker 的同步驗證，行為與 Worker 版本完全一致。
//! 主要用於單元測試與整合測試，避免在 native 環境依賴 postMessage 機制。

use bridge_types::{ShadowRequest, ShadowResponse};

use crate::{error::ShadowVmError, executor::ShadowScript, validator::ShadowValidator};

/// Native 同步 Shadow VM（測試替身，行為等同 Web Worker 版本）
///
/// 薄包裝 `ShadowValidator`，委派 `validate_sync`。
/// `ShadowValidator` 已封裝 `ShadowExecutor` 並提供統一驗證入口（GRASP Low Coupling）。
pub struct InProcessShadowVm {
    validator: ShadowValidator,
}

impl InProcessShadowVm {
    /// 載入腳本並初始化 ShadowValidator
    ///
    /// # Errors
    /// - `ShadowVmError::BytecodeLoadFailed` — 腳本格式無效或 AST 編譯失敗
    /// - `ShadowVmError::EngineInitFailed` — Rhai Engine 初始化失敗（如記憶體不足）
    pub fn new(script: ShadowScript) -> Result<Self, ShadowVmError> {
        let validator = ShadowValidator::new(script)?;
        tracing::debug!("InProcessShadowVm 初始化完成");
        Ok(Self { validator })
    }

    /// 同步執行驗證（不透過 postMessage）
    ///
    /// 語意等同於 Worker 端的 `handle_message`，委派給 `ShadowValidator::validate_sync`。
    ///
    /// # Errors
    /// 委派給 `ShadowValidator::validate_sync`，透過 `?` 傳播其錯誤。
    pub fn validate_sync(
        &mut self,
        request: &ShadowRequest,
    ) -> Result<ShadowResponse, ShadowVmError> {
        self.validator.validate_sync(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::{ShadowFrame, ShadowStatus};

    use crate::test_helpers;

    fn compile_test_script() -> ShadowScript {
        ShadowScript(
            r#"
            fn on_tick() {
                let x = input_type + input_data_int;
                x
            }
        "#
            .to_string(),
        )
    }

    fn run_main_vm_and_get_hash(
        script: &ShadowScript,
        _inputs: &[bridge_types::PlayerInput],
        rng_state: &[u8; 16],
        tick: u64,
    ) -> [u8; 32] {
        test_helpers::run_main_vm_and_get_hash(script, rng_state, tick)
    }

    fn make_shadow_frame(
        tick: u64,
        inputs: Vec<bridge_types::PlayerInput>,
        rng_state: [u8; 16],
        ecs_mirror_hash: [u8; 32],
    ) -> ShadowFrame {
        ShadowFrame {
            tick,
            inputs,
            rng_state,
            ecs_mirror_hash,
        }
    }

    // Test A: InProcessShadowVm 可載入腳本並驗證
    #[test]
    fn in_process_loads_and_validates() {
        let script = compile_test_script();
        let result = InProcessShadowVm::new(script);
        assert!(result.is_ok());
    }

    // Test B: 正確 hash → AllMatch（行為等同 Worker）
    #[test]
    fn in_process_correct_hash_allmatches() {
        let script = compile_test_script();
        let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
        let hash = run_main_vm_and_get_hash(&script, &[], &[0u8; 16], 1);
        let frame = make_shadow_frame(1, vec![], [0u8; 16], hash);
        let resp = vm
            .validate_sync(&ShadowRequest {
                frames: vec![frame],
            })
            .unwrap();
        assert!(matches!(resp.status, ShadowStatus::AllMatch));
    }

    // Test C: 錯誤 hash → Mismatch（行為等同 Worker）
    #[test]
    fn in_process_wrong_hash_mismatch() {
        let script = compile_test_script();
        let mut vm = InProcessShadowVm::new(script).unwrap();
        let frame = make_shadow_frame(1, vec![], [0u8; 16], [0xFFu8; 32]);
        let resp = vm
            .validate_sync(&ShadowRequest {
                frames: vec![frame],
            })
            .unwrap();
        assert!(matches!(
            resp.status,
            ShadowStatus::Mismatch { tick: 1, .. }
        ));
    }

    // Test D: 無效腳本 → Err（錯誤路徑）
    #[test]
    fn in_process_invalid_script() {
        let result = InProcessShadowVm::new(ShadowScript("fn on_tick( { }".to_string()));
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(
            matches!(err, ShadowVmError::BytecodeLoadFailed(_)),
            "預期 BytecodeLoadFailed，實際 {err:?}"
        );
    }

    // Test E: 空請求 → AllMatch + 空 checked_ticks
    #[test]
    fn in_process_empty_request() {
        let script = compile_test_script();
        let mut vm = InProcessShadowVm::new(script).unwrap();
        let resp = vm.validate_sync(&ShadowRequest { frames: vec![] }).unwrap();
        assert!(matches!(resp.status, ShadowStatus::AllMatch));
        assert!(resp.checked_ticks.is_empty());
    }

    // Test F: 確定性 — 相同 request 兩次呼叫產生相同結果
    #[test]
    fn in_process_deterministic_same_request_twice() {
        let script = compile_test_script();
        let mut vm = InProcessShadowVm::new(script.clone()).unwrap();
        let hash = run_main_vm_and_get_hash(&script, &[], &[0u8; 16], 1);
        let frame = make_shadow_frame(1, vec![], [0u8; 16], hash);
        let request = ShadowRequest {
            frames: vec![frame],
        };

        let resp1 = vm.validate_sync(&request).unwrap();
        let resp2 = vm.validate_sync(&request).unwrap();

        assert_eq!(resp1.status, resp2.status);
        assert_eq!(resp1.checked_ticks, resp2.checked_ticks);
    }
}
