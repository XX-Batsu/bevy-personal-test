//! Rhai VM 沙箱執行環境
//!
//! 提供受限的 Rhai 腳本執行引擎，包含操作計數、超時檢測、
//! eval 阻擋、每幀預算等安全機制。
//!
//! # 主要型別
//! - [`SandboxedEngine`]: 沙箱化 Rhai 引擎（含 50K ops limit, 2ms timeout, eval 阻擋）
//! - [`FrameBudget`]: 每幀時間預算管理器（4ms / frame）
//! - [`ScriptError`]: 腳本執行錯誤（re-export from bridge_types）
//! - [`ScopeLimiter`]: Scope 變數數量/大小限制器（Phase 7 擴充）
//! - [`OpsCostTable`]: Bridge API 操作成本查詢表（Phase 7 擴充）
//! - [`OpsTracker`]: Bridge API 操作計數器（remaining 語意，Phase 7 擴充）
//! - [`FrameOpsMetric`]: 每幀操作歷史紀錄（10 幀滑動窗口，Phase 7 擴充）
//! - [`FrameOpsEntry`]: 每幀操作統計條目（Phase 7 擴充）
//! - [`HandleRegistry`]: VFX/Audio handle 生命週期管理（Phase 7 擴充）
//! - [`DisableReason`]: 腳本停用原因（Phase 7 擴充）
//! - [`ScriptDisabled`]: 腳本停用記錄（Phase 7 擴充）
//!
//! # 使用範例
//! ```rust,no_run
//! use vm_runtime::{SandboxedEngine, FrameBudget, ScriptError};
//!
//! let engine = SandboxedEngine::new();
//! let mut budget = FrameBudget::new();
//!
//! // 簡單執行（固定 2ms 超時）
//! let result = engine.execute("let x = 1 + 2; x");
//!
//! // 幀預算內執行（動態超時，min(2ms, remaining_ms)）
//! let result = engine.execute_with_budget("let y = 3 * 4; y", &mut budget);
//! ```

pub mod atomic_update;
pub mod bridge_api;
pub mod bridge_helpers;
pub mod bytecode_loader;
pub mod camera_module;
#[cfg(feature = "debug-mode")]
pub mod dev_tools;
pub mod dynamic_convert;
pub mod fallback;
pub mod handle_registry;
pub mod hot_update;
pub mod lifecycle;
pub mod ops_cost;
pub mod performance;
pub mod sandbox;
pub mod scope_limiter;
pub mod script_manager;

pub use atomic_update::{AtomicBatchStatus, AtomicUpdateManager, BATCH_TIMEOUT_FRAMES};
pub use bridge_api::{
    register_bridge_api, BridgeState, CameraOpQueue, EntityIdAllocator, EventQueue, SharedState,
};
pub use bridge_types::ScriptError;
pub use bytecode_loader::{BytecodeLoader, LoadError as BytecodeLoadError};
pub use camera_module::register_camera_module;
pub use dynamic_convert::{to_deterministic, to_rhai_dynamic};
pub use fallback::{
    emit_disable_notification, AnimationDefault, DisableReason, ScriptDisabled, ServerPosition,
};
pub use handle_registry::{HandleRegistry, DEFAULT_MAX_LIFETIME_FRAMES};
pub use lifecycle::{LifecycleHookName, ScriptInstance};
pub use ops_cost::{FrameOpsEntry, FrameOpsMetric, OpsCostTable, OpsTracker};
pub use sandbox::{FrameBudget, SandboxedEngine};
pub use scope_limiter::{ScopeLimitError, ScopeLimiter, SCOPE_MAX_VARS, SCOPE_MAX_VAR_BYTES};
pub use script_manager::{ScriptErrorContext, ScriptId, ScriptManager, ScriptState};

// Phase 16：效能監控（無條件公開）
pub use performance::{
    record_script_timing, AutoDisableManager, AutoDisableState, DiagnosticsResource,
    ScriptDiagnostics,
};

// Phase 16：OTA 熱更新（無條件公開）
pub use hot_update::{
    BatchStatus, HotUpdateManager, MultiScriptFragment, MultiScriptUpdateBatch, UpdateAck,
    UpdateBuffer, UpdateError, OTA_CHUNK_SIZE, OTA_REASSEMBLY_TIMEOUT_US,
};

// Phase 16：共用重載型別（debug-mode：native + WASM 皆可用）
#[cfg(feature = "debug-mode")]
pub use dev_tools::{ReloadEvent, ReloadResult};

// Phase 16：File Watcher（dev-file-watcher：native-only）
#[cfg(all(feature = "dev-file-watcher", not(target_arch = "wasm32")))]
pub use dev_tools::file_watcher::{FileWatcher, WatcherError};

// Phase 16：Debug Console（debug-mode）
#[cfg(feature = "debug-mode")]
pub use dev_tools::console::{
    script_eval, script_eval_in, script_list, script_perf, script_vars, set_frame_ops_metric_ref,
    set_sandboxed_engine_ref, set_script_manager_ref,
};

#[cfg(test)]
mod smoke {
    use super::*;
    use deterministic::SoftF32;

    #[test]
    fn phase8_script_manager_compiles() {
        let _sm = ScriptManager::new();
    }

    #[test]
    fn phase8_tick_all_signature() {
        // 驗證 tick_all 四參數簽名（dt, budget, engine）— 對齊 Phase 8 實際產出
        let _: fn(
            &mut ScriptManager,
            SoftF32,
            &mut FrameBudget,
            &SandboxedEngine,
        ) -> Vec<ScriptErrorContext> = ScriptManager::tick_all;
    }

    #[test]
    fn phase5_bytecode_load_sig_exists() {
        // 驗證 bytecode_compiler::load 符號可達（參數名: verifying_key, decryption_key）
        type BytecodeLoadFn = fn(
            &[u8],
            &[u8; 32],
            &[u8; 32],
        ) -> Result<
            (bytecode_compiler::ScriptMetadata, rhai::AST),
            bytecode_compiler::LoadError,
        >;
        let _: BytecodeLoadFn = bytecode_compiler::load;
    }

    #[test]
    fn phase6_clock_trait_reachable() {
        use deterministic::Clock;
        fn _uses_clock<C: Clock>(c: &C) {
            let _: u64 = c.now_micros();
        }
    }

    #[test]
    fn phase6_sandbox_engine_reachable() {
        fn _uses_engine(_e: &SandboxedEngine) {}
        fn _uses_budget(_b: &FrameBudget) {}
    }

    #[test]
    fn phase5_load_error_variants() {
        fn _check_variants(e: bytecode_compiler::LoadError) {
            match e {
                bytecode_compiler::LoadError::Format(_) => {}
                bytecode_compiler::LoadError::SignatureVerificationFailed => {}
                bytecode_compiler::LoadError::DecryptionFailed(_) => {}
                _ => {}
            }
        }
    }

    #[test]
    fn phase8_load_script_signature() {
        // 驗證 load_script 四參數簽名（id, ast, priority, engine）— 對齊 Phase 8 實際產出
        let _: fn(
            &mut ScriptManager,
            ScriptId,
            rhai::AST,
            u8,
            &SandboxedEngine,
        ) -> Result<(), ScriptErrorContext> = ScriptManager::load_script;
    }
}
