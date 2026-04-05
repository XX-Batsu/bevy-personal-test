// engine/shadow_vm/src/command.rs

use bridge_types::{ShadowInit, ShadowRequest};
use serde::{Deserialize, Serialize};

/// 主線程 → Shadow Worker 的統一命令封裝。
/// Phase 15 新增型別（非 Phase 14 產出）。
/// 不標記 #[repr(C)]：含 Vec 的 ShadowInit/ShadowRequest 非 FFI-safe。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ShadowCommand {
    /// Worker 初始化（對應 Phase 14 handle_init）
    Init(ShadowInit),
    /// 驗證請求（對應 Phase 14 handle_message）
    Validate(ShadowRequest),
    /// 觸發 Rhai Engine 重建以釋放累積記憶體（Phase 15 新增）
    Cleanup,
}
