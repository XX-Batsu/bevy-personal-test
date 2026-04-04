//! ShadowValidator — hash 比對邏輯
//!
//! 呼叫 ShadowExecutor::replay 取得每幀 hash，逐幀與 ShadowFrame.ecs_mirror_hash 比對，
//! 組裝為 ShadowResponse（GRASP Information Expert）。

use bridge_types::{ShadowRequest, ShadowResponse, ShadowStatus};

use crate::{
    error::ShadowVmError,
    executor::{ShadowExecutor, ShadowScript},
};

/// Shadow VM 驗證器
///
/// 組合 ShadowExecutor，負責 hash 比對與 ShadowResponse 組裝。
/// Executor 負責「重播 + hash 計算」，Validator 負責「hash 比對 + 結果組裝」（GRASP High Cohesion）。
pub struct ShadowValidator {
    executor: ShadowExecutor,
}

impl ShadowValidator {
    /// 建立 ShadowValidator，內含已初始化的 ShadowExecutor
    ///
    /// # Errors
    /// - `ShadowVmError::BytecodeLoadFailed` — 腳本格式無效或 AST 編譯失敗
    /// - `ShadowVmError::EngineInitFailed` — Engine 初始化失敗
    pub fn new(script: ShadowScript) -> Result<Self, ShadowVmError> {
        let executor = ShadowExecutor::new(script)?;
        Ok(Self { executor })
    }

    /// 驗證 ShadowRequest，回傳 ShadowResponse
    ///
    /// 內部流程：
    /// 1. 呼叫 executor.replay(&request.frames) 取得每幀 hash（Vec<[u8; 32]>）
    /// 2. 逐幀比對 replay hash 與 frame.ecs_mirror_hash
    /// 3. 若不匹配，立即回傳 Mismatch（含已檢查的 tick 清單）
    /// 4. 全部通過時回傳 AllMatch
    ///
    /// # Errors
    /// - 傳播 executor.replay() 的錯誤（如 ScriptError）
    pub fn validate_sync(
        &mut self,
        request: &ShadowRequest,
    ) -> Result<ShadowResponse, ShadowVmError> {
        // 1. 呼叫 executor.replay 取得每幀 hash，executor 錯誤以 ? 傳播
        let hashes = self.executor.replay(&request.frames)?;

        // 2. 逐幀比對 replay hash 與 frame.ecs_mirror_hash
        let mut checked_ticks = Vec::new();
        for (frame, actual_hash) in request.frames.iter().zip(hashes.iter()) {
            // checked_ticks 在比對前推入，確保 Mismatch 回傳時包含不匹配的 tick
            checked_ticks.push(frame.tick);
            if *actual_hash != frame.ecs_mirror_hash {
                return Ok(ShadowResponse {
                    status: ShadowStatus::Mismatch {
                        tick: frame.tick,
                        expected_hash: frame.ecs_mirror_hash,
                        actual_hash: *actual_hash,
                    },
                    checked_ticks,
                });
            }
        }

        // 3. 全部通過 → AllMatch
        Ok(ShadowResponse {
            status: ShadowStatus::AllMatch,
            checked_ticks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::{PlayerInput, ShadowFrame};

    use crate::executor::ShadowScript;
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
        _inputs: &[PlayerInput],
        rng_state: &[u8; 16],
        tick: u64,
    ) -> [u8; 32] {
        test_helpers::run_main_vm_and_get_hash(script, rng_state, tick)
    }

    fn make_shadow_frame(
        tick: u64,
        inputs: Vec<PlayerInput>,
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

    // Test A: 正確 hash → AllMatch
    #[test]
    fn validator_correct_hash_allmatches() {
        let script = compile_test_script();
        let mut validator = ShadowValidator::new(script.clone()).unwrap();
        let hash = run_main_vm_and_get_hash(&script, &[], &[0u8; 16], 1);
        let frame = make_shadow_frame(1, vec![], [0u8; 16], hash);
        let resp = validator
            .validate_sync(&ShadowRequest {
                frames: vec![frame],
            })
            .unwrap();
        assert!(matches!(resp.status, ShadowStatus::AllMatch));
        assert_eq!(resp.checked_ticks, vec![1]);
    }

    // Test B: 單幀 hash 不匹配 → 正確的 Mismatch 資訊
    #[test]
    fn validator_single_frame_mismatch() {
        let script = compile_test_script();
        let mut validator = ShadowValidator::new(script).unwrap();
        let wrong_hash = [0xFFu8; 32];
        let frame = make_shadow_frame(5, vec![], [0u8; 16], wrong_hash);
        let resp = validator
            .validate_sync(&ShadowRequest {
                frames: vec![frame],
            })
            .unwrap();
        match resp.status {
            ShadowStatus::Mismatch {
                tick,
                expected_hash,
                ..
            } => {
                assert_eq!(tick, 5);
                assert_eq!(expected_hash, wrong_hash);
            }
            other => panic!("預期 Mismatch，實際 {other:?}"),
        }
    }

    // Test C: 多幀第 2 幀不匹配
    #[test]
    fn validator_second_frame_mismatch_checked_ticks() {
        let script = compile_test_script();
        let mut validator = ShadowValidator::new(script.clone()).unwrap();
        let rng = [0u8; 16];
        // 幀 1：正確 hash
        let hash_1 = run_main_vm_and_get_hash(&script, &[], &rng, 1);
        let frame1 = make_shadow_frame(1, vec![], rng, hash_1);
        // 幀 2：錯誤 hash
        let wrong_hash = [0xFFu8; 32];
        let frame2 = make_shadow_frame(2, vec![], rng, wrong_hash);
        let resp = validator
            .validate_sync(&ShadowRequest {
                frames: vec![frame1, frame2],
            })
            .unwrap();
        match resp.status {
            ShadowStatus::Mismatch {
                tick,
                expected_hash,
                ..
            } => {
                assert_eq!(tick, 2);
                assert_eq!(expected_hash, wrong_hash);
            }
            other => panic!("預期 Mismatch {{ tick: 2 }}，實際 {other:?}"),
        }
        // checked_ticks 應包含 tick 1（通過）和 tick 2（不匹配但已檢查）
        assert_eq!(resp.checked_ticks, vec![1, 2]);
    }

    // Test D: 無效腳本 → ShadowValidator::new 回傳 Err
    #[test]
    fn validator_invalid_script_returns_error() {
        let invalid_script = ShadowScript("fn on_tick( { }".to_string()); // 語法錯誤
        let result = ShadowValidator::new(invalid_script);
        assert!(result.is_err(), "預期無效腳本導致 new() 失敗");
        let err = result.err().unwrap();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("Bytecode 載入失敗") || err_msg.contains("初始化失敗"),
            "預期 BytecodeLoadFailed 或 EngineInitFailed，實際：{err_msg}"
        );
    }

    // Test E: 空 frames → AllMatch
    #[test]
    fn validator_empty_request() {
        let script = compile_test_script();
        let mut validator = ShadowValidator::new(script).unwrap();
        let resp = validator
            .validate_sync(&ShadowRequest { frames: vec![] })
            .unwrap();
        assert!(matches!(resp.status, ShadowStatus::AllMatch));
        assert!(resp.checked_ticks.is_empty());
    }

    // Test F: executor replay 失敗 → Err(ShadowVmError::ScriptError)
    #[test]
    fn validator_replay_failed_returns_err() {
        // 語法正確但呼叫不存在函數 — on_tick 不存在會導致 ScriptError
        let bad_script = ShadowScript(
            r#"
            fn not_on_tick() {
                42
            }
        "#
            .to_string(),
        );
        let mut validator = ShadowValidator::new(bad_script).unwrap();
        let frame = make_shadow_frame(1, vec![], [0u8; 16], [0u8; 32]);
        let result = validator.validate_sync(&ShadowRequest {
            frames: vec![frame],
        });
        assert!(result.is_err(), "預期 executor replay 失敗導致 Err");
        let err = result.err().unwrap();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("Script 執行錯誤"),
            "預期 ScriptError，實際：{err_msg}"
        );
    }

    // Test: 多幀全部正確
    #[test]
    fn validator_multiple_frames_all_match() {
        let script = compile_test_script();
        let mut validator = ShadowValidator::new(script.clone()).unwrap();
        let rng = [0u8; 16];
        let frames: Vec<ShadowFrame> = (1u64..=4)
            .map(|tick| {
                let hash = run_main_vm_and_get_hash(&script, &[], &rng, tick);
                make_shadow_frame(tick, vec![], rng, hash)
            })
            .collect();
        let resp = validator
            .validate_sync(&ShadowRequest {
                frames: frames.clone(),
            })
            .unwrap();
        assert!(matches!(resp.status, ShadowStatus::AllMatch));
        assert_eq!(resp.checked_ticks, vec![1, 2, 3, 4]);
    }
}
