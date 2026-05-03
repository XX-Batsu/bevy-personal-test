//! 攝影機特效 Bridge Op — 提供 VM → Bevy 的攝影機操作型別。
//!
//! 設計規格：`docs/design/2026-04-22-camera-effects-design.md` §3.8
//!
//! ## `#[repr(C)]` 標註選擇
//!
//! - `CameraHandle(u32)` 標 `#[repr(C)]`：純 newtype, 對齊 `lib.rs` ## 確定性保證
//!   規則「Handle newtype struct 使用 `#[repr(C)]`」。
//! - `CameraBridgeOp` enum **不**標 `#[repr(C)]`：雖然 variants 全為 primitive，
//!   但 enum discriminant 在 Rust 不擔保 ABI 穩定，序列化穩定性由 `bincode` 保證
//!   （對齊 `lib.rs` ## 確定性保證 規則「含 heap-allocated 欄位的 struct/enum
//!   不標 `#[repr(C)]`」的精神 — primitive enum 仍依 bincode 而非 layout 穩定）。
//!
//! ## 浮點型別選擇
//!
//! `CameraBridgeOp` 欄位使用 `f32`：攝影機屬渲染層子系統（README.md 允許 native float），
//! 對齊下游 Bevy `Transform` / `Projection` 欄位型別，不參與 state hash，無確定性需求。

use serde::{Deserialize, Serialize};

/// VM-facing camera handle。
///
/// `0` 為 broadcast sentinel：flush 層會將 `handle == 0` 的 op 廣播至所有
/// 已註冊 cameras（轉為 Phase A event 的 `camera: None`）。
///
/// `1..=u32::MAX` 為合法 handle，由 `bevy_runtime::camera::CameraRegistry` 維護。
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(C)]
pub struct CameraHandle(pub u32);

impl CameraHandle {
    /// Broadcast sentinel：發送至所有已註冊 cameras。
    pub const BROADCAST: CameraHandle = CameraHandle(0);

    /// 是否為 broadcast handle。
    pub fn is_broadcast(self) -> bool {
        self.0 == 0
    }
}

/// VM → Bevy 攝影機操作。
///
/// 由 Rhai 腳本透過 `camera_shake` / `camera_push_override` 等 native function
/// 產生，push 至 `BridgeState.camera_op_queue`，由 `flush_camera_ops` system
/// 在 FixedUpdate.FlushBridgeEvents stage 消費並轉為 Phase A 的
/// `CameraOverrideRequest` / `CameraShakeRequest` event。
///
/// Sentinel 編碼（在 flush 層套用）：
/// - `zoom <= 0.0`     → `target_zoom = None`（不改變 zoom）
/// - `duration < 0.0`  → `duration = None`（無期限）；`= 0.0` → `Some(0.0)`
/// - `dir_x = 0.0 && dir_y = 0.0` → `direction = None`（omnidirectional）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CameraBridgeOp {
    /// 觸發震動，對應 Phase A `CameraShakeOp::Add`。
    Shake {
        handle: u32,
        trauma: f32,
        dir_x: f32,
        dir_y: f32,
        max_strength: f32,
        decay_rate: f32,
        direction_bias: f32,
        perpendicular_damping: f32,
    },
    /// Push 一個 override，對應 Phase A `CameraOverrideOp::Push`。
    PushOverride {
        handle: u32,
        x: f32,
        y: f32,
        zoom: f32,
        speed: f32,
        priority: i32,
        duration: f32,
    },
    /// 移除最高 priority override，對應 Phase A `CameraOverrideOp::PopTop`。
    PopOverride { handle: u32 },
    /// 清空 override stack，對應 Phase A `CameraOverrideOp::Clear`。
    ClearOverrides { handle: u32 },
    /// 清空 shake entries，對應 Phase A `CameraShakeOp::Clear`。
    ClearShakes { handle: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_handle_broadcast_semantics() {
        assert_eq!(CameraHandle::BROADCAST.0, 0);
        assert!(CameraHandle::BROADCAST.is_broadcast());
        assert!(!CameraHandle(1).is_broadcast());
        assert!(!CameraHandle(u32::MAX).is_broadcast());
    }

    #[test]
    fn camera_bridge_op_shake_serde_roundtrip() {
        let op = CameraBridgeOp::Shake {
            handle: 1,
            trauma: 0.5,
            dir_x: 1.0,
            dir_y: 0.0,
            max_strength: 25.0,
            decay_rate: 2.0,
            direction_bias: 1.5,
            perpendicular_damping: 0.3,
        };
        let bytes = bincode::serialize(&op).unwrap();
        let decoded: CameraBridgeOp = bincode::deserialize(&bytes).unwrap();
        assert_eq!(op, decoded);
    }

    #[test]
    fn camera_bridge_op_push_override_serde_roundtrip() {
        let op = CameraBridgeOp::PushOverride {
            handle: 2,
            x: 100.0,
            y: 200.0,
            zoom: -1.0, // sentinel for None (resolved at flush layer)
            speed: 5.0,
            priority: 50,
            duration: -1.0, // sentinel for None
        };
        let bytes = bincode::serialize(&op).unwrap();
        let decoded: CameraBridgeOp = bincode::deserialize(&bytes).unwrap();
        assert_eq!(op, decoded);
    }

    #[test]
    fn camera_bridge_op_pop_override_serde_roundtrip() {
        let op = CameraBridgeOp::PopOverride { handle: 0 };
        let bytes = bincode::serialize(&op).unwrap();
        let decoded: CameraBridgeOp = bincode::deserialize(&bytes).unwrap();
        assert_eq!(op, decoded);
    }

    #[test]
    fn camera_bridge_op_clear_overrides_serde_roundtrip() {
        let op = CameraBridgeOp::ClearOverrides { handle: 5 };
        let bytes = bincode::serialize(&op).unwrap();
        let decoded: CameraBridgeOp = bincode::deserialize(&bytes).unwrap();
        assert_eq!(op, decoded);
    }

    #[test]
    fn camera_bridge_op_clear_shakes_serde_roundtrip() {
        let op = CameraBridgeOp::ClearShakes { handle: 7 };
        let bytes = bincode::serialize(&op).unwrap();
        let decoded: CameraBridgeOp = bincode::deserialize(&bytes).unwrap();
        assert_eq!(op, decoded);
    }

    #[test]
    fn camera_handle_serde_roundtrip() {
        let h = CameraHandle(42);
        let bytes = bincode::serialize(&h).unwrap();
        let decoded: CameraHandle = bincode::deserialize(&bytes).unwrap();
        assert_eq!(h, decoded);
    }
}
