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

pub mod bridge_api;
pub mod dynamic_convert;
pub mod fallback;
pub mod handle_registry;
pub mod lifecycle;
pub mod ops_cost;
pub mod sandbox;
pub mod scope_limiter;
pub mod script_manager;

pub use bridge_api::{
    register_bridge_api, BridgeState, EntityIdAllocator, EventQueue, SharedState,
};
pub use bridge_types::ScriptError;
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
