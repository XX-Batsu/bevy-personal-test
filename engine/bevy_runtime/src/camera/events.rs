//! 攝影機特效的 Event API。
//!
//! 設計規格：`docs/design/2026-04-22-camera-effects-design.md` §3.5

use bevy::prelude::*;

use super::override_stack::{OverrideId, PushOverrideParams};

/// 請求對一或多個 camera 的 `CameraOverrideStack` 執行操作。
/// `camera = None` 表示套用至所有含 `CameraOverrideStack` 的 camera entity。
#[derive(Event, Debug, Clone)]
pub struct CameraOverrideRequest {
    pub camera: Option<Entity>,
    pub op: CameraOverrideOp,
}

#[derive(Debug, Clone)]
pub enum CameraOverrideOp {
    Push(PushOverrideParams),
    RemoveById(OverrideId),
    PopTop,
    Clear,
}

use super::shake::{ShakeId, ShakeParams};

/// 請求對一或多個 camera 的 `CameraShake` 執行操作。
/// `camera = None` 表示套用至所有含 `CameraShake` 的 camera entity。
#[derive(Event, Debug, Clone)]
pub struct CameraShakeRequest {
    pub camera: Option<Entity>,
    pub op: CameraShakeOp,
}

#[derive(Debug, Clone)]
pub enum CameraShakeOp {
    Add(ShakeParams),
    RemoveById(ShakeId),
    Clear,
}
