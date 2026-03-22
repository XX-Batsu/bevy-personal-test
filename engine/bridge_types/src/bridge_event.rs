//! VM → Bevy 單向事件列舉
//!
//! `BridgeEvent` 為 VM 執行腳本時產生的事件，收集至 `BridgeEventQueue`，
//! 在 Bevy Last schedule stage 由 `flush_bridge_events` system 消費。
//!
//! ## Determinism 合規
//!
//! - Entity 操作分類（`SpawnEntity`..`SetVisibility`）的欄位型別僅限
//!   `i64`、`bool`、`EntityId`、`SoftVec3`，禁止 `f64`。
//! - UI / Animation 分類中的 `f64` 欄位屬渲染層輸出，
//!   不參與 state hash 計算，符合 Determinism Rules。

use deterministic::SoftVec3;
use serde::{Deserialize, Serialize};

use crate::{DynamicValue, EffectHandle, EntityId, SoundHandle};

/// VM -> Bevy 單向事件
/// 每幀由 VM 執行腳本時產生，收集至 BridgeEventQueue，
/// 在 Bevy Last schedule stage 由 flush_bridge_events system 消費
///
/// f64 欄位出現於 UI / Animation 分類中，屬渲染層輸出，
/// 不參與 state hash 計算，符合 Determinism Rules。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BridgeEvent {
    // ═══════════════════════════════════════════
    // Entity 操作（對應 Bridge API §3.1）
    // ═══════════════════════════════════════════
    /// 建立實體，type_id 對應 GameEntityBundle 工廠
    SpawnEntity { type_id: i64 },

    /// 移除實體
    DespawnEntity { eid: EntityId },

    /// 設定位置（SoftVec3 確保確定性）
    SetTransform { eid: EntityId, pos: SoftVec3 },

    /// 設定旋轉（Euler angles，SoftVec3）
    SetRotation { eid: EntityId, euler: SoftVec3 },

    /// 設定縮放
    SetScale { eid: EntityId, scale: SoftVec3 },

    /// 設定可見性
    SetVisibility { eid: EntityId, visible: bool },

    // ═══════════════════════════════════════════
    // VFX / Audio（對應 Bridge API §3.2）
    // ═══════════════════════════════════════════
    /// 播放特效，回傳 EffectHandle（由 Bridge 層分配）
    PlayVfx { vfx_id: i64, pos: SoftVec3 },

    /// 停止特效
    StopVfx { handle: EffectHandle },

    /// 播放 2D 音效
    PlaySound { sound_id: i64 },

    /// 播放 3D 定位音效
    PlaySoundAt { sound_id: i64, pos: SoftVec3 },

    /// 停止音效
    StopSound { handle: SoundHandle },

    // ═══════════════════════════════════════════
    // UI（對應 Bridge API §3.3）
    // 渲染層輸出，f64 欄位不參與 deterministic simulation
    // ═══════════════════════════════════════════
    /// 顯示對話框
    ShowDialog { dialog_id: i64 },

    /// 隱藏對話框
    HideDialog { dialog_id: i64 },

    /// 更新 HUD 數值（key-value 形式）
    UpdateHud { key: String, value: DynamicValue },

    /// 設定血條（f64: 渲染層，不參與 state hash）
    SetHealthBar {
        eid: EntityId,
        current: f64,
        max: f64,
    },

    /// 顯示傷害數字浮動文字（f64: 世界座標，渲染層轉換為螢幕座標）
    ShowDamageNumber { x: f64, y: f64, value: i64 },

    /// 顯示 Toast 提示
    ShowToast { msg: String, duration_ms: i64 },

    // ═══════════════════════════════════════════
    // Animation（對應 Bridge API §3.4）
    // 渲染層輸出，weight: f64 不參與 deterministic simulation
    // ═══════════════════════════════════════════
    /// 播放循環動畫
    PlayAnimation { eid: EntityId, anim_id: i64 },

    /// 播放一次性動畫
    PlayAnimationOnce { eid: EntityId, anim_id: i64 },

    /// 停止動畫
    StopAnimation { eid: EntityId },

    /// 動畫混合（weight: f64 為渲染層插值參數）
    BlendAnimation {
        eid: EntityId,
        anim_a: i64,
        anim_b: i64,
        weight: f64,
    },

    // ═══════════════════════════════════════════
    // Network（對應 Bridge API §3.6）
    // ═══════════════════════════════════════════
    /// 送出預測輸入至 netcode 層
    SendPrediction { input_type: i64, data: DynamicValue },

    /// 從 ECS Mirror 請求 server state
    RequestState { key: String },
}

impl BridgeEvent {
    /// 回傳此 event 關聯的 EntityId（若有）。
    /// SpawnEntity 在 flush 時才分配 eid，故回傳 None。
    /// UI/Network/VFX（無 eid 欄位）回傳 None。
    pub fn entity_id(&self) -> Option<EntityId> {
        match self {
            Self::DespawnEntity { eid }
            | Self::SetTransform { eid, .. }
            | Self::SetRotation { eid, .. }
            | Self::SetScale { eid, .. }
            | Self::SetVisibility { eid, .. }
            | Self::SetHealthBar { eid, .. }
            | Self::PlayAnimation { eid, .. }
            | Self::PlayAnimationOnce { eid, .. }
            | Self::StopAnimation { eid }
            | Self::BlendAnimation { eid, .. } => Some(*eid),
            _ => None,
        }
    }
}

/// 編譯期斷言：BridgeEvent 恰好 23 個 variant
/// 若 variant 數量變更，此測試將無法編譯
#[cfg(test)]
const _: () = {
    // 利用 match exhaustiveness 檢查 variant 數量
    // 若新增或移除 variant，此處會編譯錯誤
    const fn _assert_23_variants(e: &BridgeEvent) -> u8 {
        match e {
            BridgeEvent::SpawnEntity { .. } => 0,
            BridgeEvent::DespawnEntity { .. } => 1,
            BridgeEvent::SetTransform { .. } => 2,
            BridgeEvent::SetRotation { .. } => 3,
            BridgeEvent::SetScale { .. } => 4,
            BridgeEvent::SetVisibility { .. } => 5,
            BridgeEvent::PlayVfx { .. } => 6,
            BridgeEvent::StopVfx { .. } => 7,
            BridgeEvent::PlaySound { .. } => 8,
            BridgeEvent::PlaySoundAt { .. } => 9,
            BridgeEvent::StopSound { .. } => 10,
            BridgeEvent::ShowDialog { .. } => 11,
            BridgeEvent::HideDialog { .. } => 12,
            BridgeEvent::UpdateHud { .. } => 13,
            BridgeEvent::SetHealthBar { .. } => 14,
            BridgeEvent::ShowDamageNumber { .. } => 15,
            BridgeEvent::ShowToast { .. } => 16,
            BridgeEvent::PlayAnimation { .. } => 17,
            BridgeEvent::PlayAnimationOnce { .. } => 18,
            BridgeEvent::StopAnimation { .. } => 19,
            BridgeEvent::BlendAnimation { .. } => 20,
            BridgeEvent::SendPrediction { .. } => 21,
            BridgeEvent::RequestState { .. } => 22,
        }
    }
};

#[cfg(test)]
mod tests {
    use super::*;
    use deterministic::SoftF32;

    fn soft_vec3(x: f32, y: f32, z: f32) -> SoftVec3 {
        SoftVec3 {
            x: SoftF32::from_f32(x),
            y: SoftF32::from_f32(y),
            z: SoftF32::from_f32(z),
        }
    }

    fn round_trip(event: &BridgeEvent) {
        let bytes = bincode::serialize(event).unwrap();
        let decoded: BridgeEvent = bincode::deserialize(&bytes).unwrap();
        assert_eq!(*event, decoded);
    }

    #[test]
    fn bridge_event_entity_ops_round_trip() {
        round_trip(&BridgeEvent::SpawnEntity { type_id: 1 });
        round_trip(&BridgeEvent::DespawnEntity { eid: EntityId(5) });
        round_trip(&BridgeEvent::SetTransform {
            eid: EntityId(1),
            pos: soft_vec3(1.0, 2.0, 3.0),
        });
        round_trip(&BridgeEvent::SetRotation {
            eid: EntityId(1),
            euler: soft_vec3(0.0, 90.0, 0.0),
        });
        round_trip(&BridgeEvent::SetScale {
            eid: EntityId(1),
            scale: soft_vec3(1.0, 1.0, 1.0),
        });
        round_trip(&BridgeEvent::SetVisibility {
            eid: EntityId(1),
            visible: false,
        });
    }

    #[test]
    fn bridge_event_vfx_audio_round_trip() {
        round_trip(&BridgeEvent::PlayVfx {
            vfx_id: 10,
            pos: soft_vec3(0.0, 1.0, 0.0),
        });
        round_trip(&BridgeEvent::StopVfx {
            handle: EffectHandle(1),
        });
        round_trip(&BridgeEvent::PlaySound { sound_id: 5 });
        round_trip(&BridgeEvent::PlaySoundAt {
            sound_id: 5,
            pos: soft_vec3(3.0, 0.0, 0.0),
        });
        round_trip(&BridgeEvent::StopSound {
            handle: SoundHandle(2),
        });
    }

    #[test]
    fn bridge_event_ui_round_trip() {
        round_trip(&BridgeEvent::ShowDialog { dialog_id: 1 });
        round_trip(&BridgeEvent::HideDialog { dialog_id: 1 });
        round_trip(&BridgeEvent::UpdateHud {
            key: "score".to_string(),
            value: DynamicValue::Int(100),
        });
        round_trip(&BridgeEvent::SetHealthBar {
            eid: EntityId(1),
            current: 50.0,
            max: 100.0,
        });
        round_trip(&BridgeEvent::ShowDamageNumber {
            x: 10.0,
            y: 20.0,
            value: 999,
        });
        round_trip(&BridgeEvent::ShowToast {
            msg: "Hello".to_string(),
            duration_ms: 3000,
        });
    }

    #[test]
    fn bridge_event_animation_round_trip() {
        round_trip(&BridgeEvent::PlayAnimation {
            eid: EntityId(1),
            anim_id: 3,
        });
        round_trip(&BridgeEvent::PlayAnimationOnce {
            eid: EntityId(1),
            anim_id: 5,
        });
        round_trip(&BridgeEvent::StopAnimation { eid: EntityId(1) });
        round_trip(&BridgeEvent::BlendAnimation {
            eid: EntityId(1),
            anim_a: 1,
            anim_b: 2,
            weight: 0.5,
        });
    }

    #[test]
    fn bridge_event_network_round_trip() {
        round_trip(&BridgeEvent::SendPrediction {
            input_type: 1,
            data: DynamicValue::Int(42),
        });
        round_trip(&BridgeEvent::RequestState {
            key: "hp".to_string(),
        });
    }

    #[test]
    fn bridge_event_spawn_entity_zero_type() {
        round_trip(&BridgeEvent::SpawnEntity { type_id: 0 });
    }

    #[test]
    fn bridge_event_empty_string_fields() {
        round_trip(&BridgeEvent::UpdateHud {
            key: String::new(),
            value: DynamicValue::Unit,
        });
        round_trip(&BridgeEvent::ShowToast {
            msg: String::new(),
            duration_ms: 0,
        });
    }

    #[test]
    fn bridge_event_entity_id_some() {
        let eid = EntityId(42);
        assert_eq!(BridgeEvent::DespawnEntity { eid }.entity_id(), Some(eid));
        assert_eq!(
            BridgeEvent::SetTransform {
                eid,
                pos: soft_vec3(0.0, 0.0, 0.0)
            }
            .entity_id(),
            Some(eid)
        );
        assert_eq!(
            BridgeEvent::SetRotation {
                eid,
                euler: soft_vec3(0.0, 0.0, 0.0)
            }
            .entity_id(),
            Some(eid)
        );
        assert_eq!(
            BridgeEvent::SetScale {
                eid,
                scale: soft_vec3(1.0, 1.0, 1.0)
            }
            .entity_id(),
            Some(eid)
        );
        assert_eq!(
            BridgeEvent::SetVisibility { eid, visible: true }.entity_id(),
            Some(eid)
        );
        assert_eq!(
            BridgeEvent::SetHealthBar {
                eid,
                current: 50.0,
                max: 100.0
            }
            .entity_id(),
            Some(eid)
        );
        assert_eq!(
            BridgeEvent::PlayAnimation { eid, anim_id: 1 }.entity_id(),
            Some(eid)
        );
        assert_eq!(
            BridgeEvent::PlayAnimationOnce { eid, anim_id: 1 }.entity_id(),
            Some(eid)
        );
        assert_eq!(BridgeEvent::StopAnimation { eid }.entity_id(), Some(eid));
        assert_eq!(
            BridgeEvent::BlendAnimation {
                eid,
                anim_a: 1,
                anim_b: 2,
                weight: 0.5
            }
            .entity_id(),
            Some(eid)
        );
    }

    #[test]
    fn bridge_event_entity_id_none() {
        assert_eq!(BridgeEvent::SpawnEntity { type_id: 1 }.entity_id(), None);
        assert_eq!(
            BridgeEvent::PlayVfx {
                vfx_id: 1,
                pos: soft_vec3(0.0, 0.0, 0.0)
            }
            .entity_id(),
            None
        );
        assert_eq!(
            BridgeEvent::StopVfx {
                handle: EffectHandle(1)
            }
            .entity_id(),
            None
        );
        assert_eq!(BridgeEvent::PlaySound { sound_id: 1 }.entity_id(), None);
        assert_eq!(
            BridgeEvent::PlaySoundAt {
                sound_id: 1,
                pos: soft_vec3(0.0, 0.0, 0.0)
            }
            .entity_id(),
            None
        );
        assert_eq!(
            BridgeEvent::StopSound {
                handle: SoundHandle(1)
            }
            .entity_id(),
            None
        );
        assert_eq!(BridgeEvent::ShowDialog { dialog_id: 1 }.entity_id(), None);
        assert_eq!(BridgeEvent::HideDialog { dialog_id: 1 }.entity_id(), None);
        assert_eq!(
            BridgeEvent::UpdateHud {
                key: "k".to_string(),
                value: DynamicValue::Int(0)
            }
            .entity_id(),
            None
        );
        assert_eq!(
            BridgeEvent::ShowDamageNumber {
                x: 0.0,
                y: 0.0,
                value: 1
            }
            .entity_id(),
            None
        );
        assert_eq!(
            BridgeEvent::ShowToast {
                msg: "t".to_string(),
                duration_ms: 100
            }
            .entity_id(),
            None
        );
        assert_eq!(
            BridgeEvent::SendPrediction {
                input_type: 1,
                data: DynamicValue::Int(0)
            }
            .entity_id(),
            None
        );
        assert_eq!(
            BridgeEvent::RequestState {
                key: "k".to_string()
            }
            .entity_id(),
            None
        );
    }
}
