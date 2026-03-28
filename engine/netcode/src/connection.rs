//! 斷線重連狀態機

use bridge_types::EcsMirror;
use serde::{Deserialize, Serialize};

use crate::error::NetcodeError;
use crate::snapshot::{GameSnapshot, SnapshotBuffer};

/// 斷線超時（ticks）：30s × 60Hz = 1800
pub const FROZEN_TIMEOUT_TICKS: u64 = 1800;

/// 重試間隔（ticks）：2s × 60Hz = 120
pub const RETRY_INTERVAL_TICKS: u64 = 120;

/// 最大重同步重試次數
pub const MAX_RESYNC_RETRIES: u32 = 3;

/// 連線狀態
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// 正常連線中
    Connected,
    /// 連線凍結（等待重連）
    Frozen { frozen_since_tick: u64 },
    /// 重新同步中
    Resync {
        retry_count: u32,
        frozen_since_tick: u64,
    },
    /// 已斷線
    Disconnected,
}

/// 重連動作
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconnectAction {
    /// 嘗試重連
    AttemptReconnect,
    /// 超時
    Timeout,
}

/// 完整同步封包（server 下發）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FullSyncPacket {
    /// 幀序號
    pub tick: u64,
    /// ECS 完整狀態
    pub ecs_mirror: EcsMirror,
    /// RNG 狀態
    pub rng_state: [u8; 16],
}

/// 斷線重連狀態機
pub struct ConnectionManager {
    state: ConnectionState,
    last_retry_tick: u64,
    force_reload: bool,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            state: ConnectionState::Connected,
            last_retry_tick: 0,
            force_reload: false,
        }
    }

    /// 取得當前連線狀態
    pub fn state(&self) -> &ConnectionState {
        &self.state
    }

    /// 通知斷線
    pub fn on_disconnect(&mut self, current_tick: u64) {
        if self.state == ConnectionState::Connected {
            self.state = ConnectionState::Frozen {
                frozen_since_tick: current_tick,
            };
            tracing::warn!(tick = current_tick, "連線凍結");
        }
    }

    /// 處理重連（收到 FullSyncPacket）
    ///
    /// 驗證 hash 後將 snapshot 推入 buffer
    pub fn on_reconnect(
        &mut self,
        packet: FullSyncPacket,
        expected_hash: [u8; 32],
        snapshot_buffer: &mut SnapshotBuffer<GameSnapshot>,
        computed_hash: [u8; 32],
    ) -> Result<(), NetcodeError> {
        if computed_hash != expected_hash {
            tracing::warn!("重新同步 hash 不符");
            return Err(NetcodeError::ResyncHashMismatch);
        }

        // 推入 snapshot
        let snapshot = GameSnapshot {
            tick: packet.tick,
            ecs_mirror: packet.ecs_mirror,
            rng_state: packet.rng_state,
        };
        snapshot_buffer.clear();
        snapshot_buffer.push(packet.tick, snapshot);

        self.state = ConnectionState::Connected;
        tracing::info!("重新同步完成，恢復連線");
        Ok(())
    }

    /// 處理版本不匹配（需要強制重新載入）
    pub fn on_version_mismatch(&mut self) {
        self.force_reload = true;
        self.state = ConnectionState::Disconnected;
        tracing::warn!("版本不匹配，需要強制重新載入");
    }

    /// 是否需要強制重新載入
    pub fn needs_force_reload(&self) -> bool {
        self.force_reload
    }

    /// 每 tick 更新狀態機
    ///
    /// 回傳需要執行的動作（如果有的話）
    pub fn tick(&mut self, current_tick: u64) -> Option<ReconnectAction> {
        match &self.state {
            ConnectionState::Frozen { frozen_since_tick } => {
                let elapsed = current_tick.saturating_sub(*frozen_since_tick);
                if elapsed >= FROZEN_TIMEOUT_TICKS {
                    self.state = ConnectionState::Disconnected;
                    tracing::error!("連線超時，斷開");
                    return Some(ReconnectAction::Timeout);
                }
                // 進入 Resync 狀態
                self.state = ConnectionState::Resync {
                    retry_count: 0,
                    frozen_since_tick: *frozen_since_tick,
                };
                self.last_retry_tick = current_tick;
                Some(ReconnectAction::AttemptReconnect)
            }
            ConnectionState::Resync {
                retry_count,
                frozen_since_tick,
            } => {
                let total_elapsed = current_tick.saturating_sub(*frozen_since_tick);
                if total_elapsed >= FROZEN_TIMEOUT_TICKS {
                    self.state = ConnectionState::Disconnected;
                    tracing::error!("重同步超時，斷開");
                    return Some(ReconnectAction::Timeout);
                }

                let since_last_retry = current_tick.saturating_sub(self.last_retry_tick);
                if since_last_retry >= RETRY_INTERVAL_TICKS {
                    let new_retry = retry_count + 1;
                    if new_retry > MAX_RESYNC_RETRIES {
                        self.state = ConnectionState::Disconnected;
                        tracing::error!("重同步重試次數用盡，斷開");
                        return Some(ReconnectAction::Timeout);
                    }
                    self.state = ConnectionState::Resync {
                        retry_count: new_retry,
                        frozen_since_tick: *frozen_since_tick,
                    };
                    self.last_retry_tick = current_tick;
                    return Some(ReconnectAction::AttemptReconnect);
                }

                None
            }
            _ => None,
        }
    }
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::EntityId;
    use deterministic::SoftF32;
    use state_hash::compute_state_hash;
    use std::collections::BTreeMap;

    fn make_ecs_mirror() -> EcsMirror {
        EcsMirror {
            entities: BTreeMap::new(),
            local_player_id: EntityId(0),
            frame_number: 0,
            delta_time: SoftF32::from_f32(0.01667),
        }
    }

    fn make_full_sync_packet(tick: u64) -> FullSyncPacket {
        FullSyncPacket {
            tick,
            ecs_mirror: make_ecs_mirror(),
            rng_state: [0u8; 16],
        }
    }

    #[test]
    fn new_is_connected() {
        let cm = ConnectionManager::new();
        assert_eq!(*cm.state(), ConnectionState::Connected);
    }

    #[test]
    fn disconnect_transitions_to_frozen() {
        let mut cm = ConnectionManager::new();
        cm.on_disconnect(100);
        assert_eq!(
            *cm.state(),
            ConnectionState::Frozen {
                frozen_since_tick: 100
            }
        );
    }

    #[test]
    fn frozen_tick_triggers_resync() {
        let mut cm = ConnectionManager::new();
        cm.on_disconnect(100);
        let action = cm.tick(100);
        assert_eq!(action, Some(ReconnectAction::AttemptReconnect));
        assert!(matches!(*cm.state(), ConnectionState::Resync { .. }));
    }

    #[test]
    fn resync_retry_after_interval() {
        let mut cm = ConnectionManager::new();
        cm.on_disconnect(100);
        cm.tick(100); // → Resync(retry=0)

        // 在間隔內 tick → None
        assert_eq!(cm.tick(150), None);

        // 超過 RETRY_INTERVAL_TICKS → retry
        let action = cm.tick(220);
        assert_eq!(action, Some(ReconnectAction::AttemptReconnect));
    }

    #[test]
    fn resync_max_retries_disconnects() {
        let mut cm = ConnectionManager::new();
        cm.on_disconnect(0);
        cm.tick(0); // → Resync(retry=0)

        // 逐步重試到上限
        for i in 1..=3 {
            let tick = i as u64 * RETRY_INTERVAL_TICKS;
            let action = cm.tick(tick);
            assert_eq!(action, Some(ReconnectAction::AttemptReconnect));
        }

        // 第 4 次 → 超過 MAX_RESYNC_RETRIES → Disconnected
        let tick = 4 * RETRY_INTERVAL_TICKS;
        let action = cm.tick(tick);
        assert_eq!(action, Some(ReconnectAction::Timeout));
        assert_eq!(*cm.state(), ConnectionState::Disconnected);
    }

    #[test]
    fn frozen_timeout_disconnects() {
        let mut cm = ConnectionManager::new();
        cm.on_disconnect(0);
        // 直接跳到 FROZEN_TIMEOUT_TICKS
        let action = cm.tick(FROZEN_TIMEOUT_TICKS);
        assert_eq!(action, Some(ReconnectAction::Timeout));
        assert_eq!(*cm.state(), ConnectionState::Disconnected);
    }

    #[test]
    fn on_reconnect_success() {
        let mut cm = ConnectionManager::new();
        cm.on_disconnect(100);
        cm.tick(100); // → Resync
        let mut sb = SnapshotBuffer::new();

        let packet = make_full_sync_packet(200);
        let expected_hash =
            compute_state_hash(&packet.ecs_mirror.entities, &packet.rng_state, packet.tick);

        assert!(cm
            .on_reconnect(packet, expected_hash, &mut sb, expected_hash)
            .is_ok());
        assert_eq!(*cm.state(), ConnectionState::Connected);
        assert!(sb.get(200).is_some());
    }

    #[test]
    fn on_reconnect_hash_mismatch() {
        let mut cm = ConnectionManager::new();
        cm.on_disconnect(100);
        cm.tick(100);
        let mut sb = SnapshotBuffer::new();

        let packet = make_full_sync_packet(200);
        let computed_hash =
            compute_state_hash(&packet.ecs_mirror.entities, &packet.rng_state, packet.tick);
        let wrong_hash = [0xFF; 32];

        assert_eq!(
            cm.on_reconnect(packet, wrong_hash, &mut sb, computed_hash),
            Err(NetcodeError::ResyncHashMismatch)
        );
    }

    #[test]
    fn connected_tick_returns_none() {
        let mut cm = ConnectionManager::new();
        assert_eq!(cm.tick(100), None);
    }

    #[test]
    fn disconnected_tick_returns_none() {
        let mut cm = ConnectionManager::new();
        cm.on_disconnect(0);
        cm.tick(FROZEN_TIMEOUT_TICKS); // → Disconnected
        assert_eq!(cm.tick(FROZEN_TIMEOUT_TICKS + 100), None);
    }

    #[test]
    fn disconnect_while_frozen_is_idempotent() {
        let mut cm = ConnectionManager::new();
        cm.on_disconnect(100);
        cm.on_disconnect(200); // 已在 Frozen，不變
        assert_eq!(
            *cm.state(),
            ConnectionState::Frozen {
                frozen_since_tick: 100
            }
        );
    }

    #[test]
    fn frozen_since_tick_preserved_in_resync() {
        let mut cm = ConnectionManager::new();
        cm.on_disconnect(42);
        cm.tick(42);
        match cm.state() {
            ConnectionState::Resync {
                frozen_since_tick, ..
            } => {
                assert_eq!(*frozen_since_tick, 42);
            }
            _ => panic!("should be Resync"),
        }
    }

    #[test]
    fn version_mismatch_sets_force_reload() {
        let mut cm = ConnectionManager::new();
        assert!(!cm.needs_force_reload());

        cm.on_version_mismatch();
        assert!(cm.needs_force_reload());
        assert_eq!(*cm.state(), ConnectionState::Disconnected);
    }

    #[test]
    fn needs_force_reload_default_false() {
        let cm = ConnectionManager::new();
        assert!(!cm.needs_force_reload());
    }
}
