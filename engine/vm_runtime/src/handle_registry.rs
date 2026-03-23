//! VFX/Audio handle 生命週期管理
//!
//! 管理 EffectHandle 與 SoundHandle 的建立、失效、過期清理。
//! 使用 BTreeMap 確保確定性迭代順序（遵守 Determinism Rules）。
//!
//! Handle 語義（對齊 vfx-audio-ui-animation.md §不變式 #4）：
//! - `stop_vfx` / `stop_sound` 使用無效 handle → 靜默忽略（fire-and-forget）
//! - `invalidate()` 對不存在/已失效 handle → 靜默忽略（冪等）
//! - Handle 超過 max_lifetime_frames → 自動過期，`sweep_expired()` 清理

use std::collections::BTreeMap;

use bridge_types::{EffectHandle, SoundHandle};

/// Handle 預設最大存活幀數（5 秒 @ 60fps）
pub const DEFAULT_MAX_LIFETIME_FRAMES: u64 = 300;

/// Handle 內部紀錄
struct HandleEntry {
    /// 建立時的幀號
    created_frame: u64,
    /// 最大存活幀數
    max_lifetime_frames: u64,
    /// 是否仍然有效
    valid: bool,
}

/// Handle 生命週期註冊表
///
/// 管理 VFX（EffectHandle）和 Audio（SoundHandle）的生命週期。
/// 使用 BTreeMap 確保確定性迭代順序（遵守 README.md Determinism Rules）。
///
/// Handle 語義（對齊 vfx-audio-ui-animation.md §錯誤處理 + §不變式 #4）：
/// - `stop_vfx` / `stop_sound` 使用無效 handle → 靜默忽略（fire-and-forget），
///   不 log、不 panic、不回傳 Err（系統契約——腳本開發者依賴此行為）
/// - `invalidate()` 對不存在/已失效 handle → 靜默忽略（冪等）
/// - Handle 超過 max_lifetime_frames → 自動過期，`sweep_expired()` 清理
///
/// fire-and-forget 決策論證（對齊 vfx-audio-ui-animation.md §決策論證）：
/// VFX/Audio 的生命週期由渲染系統管理，腳本無法可靠得知 handle 何時過期
/// （例如特效自然播放完畢），每次過期都 log 會在正常遊戲中產生大量噪音。
pub struct HandleRegistry {
    effects: BTreeMap<u32, HandleEntry>,
    sounds: BTreeMap<u32, HandleEntry>,
    next_id: u32,
}

impl Default for HandleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HandleRegistry {
    /// 建立空的 HandleRegistry
    pub fn new() -> Self {
        Self {
            effects: BTreeMap::new(),
            sounds: BTreeMap::new(),
            next_id: 0,
        }
    }

    /// 建立新的 VFX handle
    ///
    /// `next_id` 使用 `wrapping_add(1)` 遞增（debug/release 行為一致）。
    /// `next_id` 溢出行為：u32::MAX 後 wrapping add → 0。
    /// 實務上 300 frame 存活期 × 60fps ≈ 單場景不可能耗盡 4B handle。
    pub fn create_effect(&mut self, frame: u64) -> EffectHandle {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.effects.insert(
            id,
            HandleEntry {
                created_frame: frame,
                max_lifetime_frames: DEFAULT_MAX_LIFETIME_FRAMES,
                valid: true,
            },
        );
        EffectHandle(id)
    }

    /// 建立新的 Sound handle
    ///
    /// `next_id` 使用 `wrapping_add(1)` 遞增（debug/release 行為一致）。
    /// `next_id` 溢出行為同 `create_effect()`。
    pub fn create_sound(&mut self, frame: u64) -> SoundHandle {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.sounds.insert(
            id,
            HandleEntry {
                created_frame: frame,
                max_lifetime_frames: DEFAULT_MAX_LIFETIME_FRAMES,
                valid: true,
            },
        );
        SoundHandle(id)
    }

    /// 使 effect handle 失效
    /// 不存在或已失效 → 靜默忽略（冪等）
    pub fn invalidate_effect(&mut self, handle: EffectHandle) {
        if let Some(entry) = self.effects.get_mut(&handle.0) {
            entry.valid = false;
        }
    }

    /// 使 sound handle 失效
    /// 不存在或已失效 → 靜默忽略（冪等）
    pub fn invalidate_sound(&mut self, handle: SoundHandle) {
        if let Some(entry) = self.sounds.get_mut(&handle.0) {
            entry.valid = false;
        }
    }

    /// 檢查 effect handle 是否有效
    pub fn is_valid_effect(&self, handle: EffectHandle) -> bool {
        self.effects.get(&handle.0).is_some_and(|e| e.valid)
    }

    /// 檢查 sound handle 是否有效
    pub fn is_valid_sound(&self, handle: SoundHandle) -> bool {
        self.sounds.get(&handle.0).is_some_and(|e| e.valid)
    }

    /// 清理過期 handle（建議每 60 frames 呼叫一次）
    ///
    /// 保留條件：`valid && (current_frame - created_frame) < max_lifetime_frames`
    /// - frame=0 建立、max=300 → sweep(299) 保留（299 < 300）、sweep(300) 移除（300 ≮ 300）
    /// - 已 invalidate（valid=false）的 entry 無論幀數一律移除
    ///
    /// `current_frame` 保證 ≥ `created_frame`（frame 為單調遞增，由 SimulationClock 管理），
    /// 因此減法不會 underflow。
    ///
    /// Phase 9 負責排程到 Bevy `Last` schedule，每 60 frames 呼叫一次。
    pub fn sweep_expired(&mut self, current_frame: u64) {
        self.effects
            .retain(|_, e| e.valid && (current_frame - e.created_frame) < e.max_lifetime_frames);
        self.sounds
            .retain(|_, e| e.valid && (current_frame - e.created_frame) < e.max_lifetime_frames);
    }

    /// 回傳目前活躍的 effect handle 數量（供診斷用）
    pub fn effect_count(&self) -> usize {
        self.effects.len()
    }

    /// 回傳目前活躍的 sound handle 數量（供診斷用）
    pub fn sound_count(&self) -> usize {
        self.sounds.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_types::{EffectHandle, SoundHandle};

    // --- 建立 ---

    #[test]
    fn test_create_effect_valid() {
        let mut registry = HandleRegistry::new();
        let handle = registry.create_effect(0);
        assert!(registry.is_valid_effect(handle));
    }

    #[test]
    fn test_create_sound_valid() {
        let mut registry = HandleRegistry::new();
        let handle = registry.create_sound(0);
        assert!(registry.is_valid_sound(handle));
    }

    // --- 失效（invalidate） ---

    #[test]
    fn test_invalidate_effect() {
        let mut registry = HandleRegistry::new();
        let handle = registry.create_effect(0);
        assert!(registry.is_valid_effect(handle));
        registry.invalidate_effect(handle);
        assert!(!registry.is_valid_effect(handle));
    }

    #[test]
    fn test_invalidate_sound() {
        let mut registry = HandleRegistry::new();
        let handle = registry.create_sound(0);
        assert!(registry.is_valid_sound(handle));
        registry.invalidate_sound(handle);
        assert!(!registry.is_valid_sound(handle));
    }

    // --- fire-and-forget 契約（對齊不變式 #4）---

    #[test]
    fn test_invalid_handle_silently_ignored() {
        let registry = HandleRegistry::new();
        // 不存在的 handle → false，無 panic（fire-and-forget）
        assert!(!registry.is_valid_effect(EffectHandle(9999)));
        assert!(!registry.is_valid_sound(SoundHandle(9999)));
    }

    #[test]
    fn test_invalidate_nonexistent_handle() {
        let mut registry = HandleRegistry::new();
        // 不存在的 handle → 靜默忽略，不 panic
        registry.invalidate_effect(EffectHandle(9999));
        registry.invalidate_sound(SoundHandle(9999));
    }

    #[test]
    fn test_invalidate_already_invalid() {
        let mut registry = HandleRegistry::new();
        let handle = registry.create_effect(0);
        registry.invalidate_effect(handle);
        // 再次 invalidate — 冪等，不 panic
        registry.invalidate_effect(handle);
        assert!(!registry.is_valid_effect(handle));
    }

    // --- sweep 邊界 ---

    #[test]
    fn test_handle_auto_expire() {
        let mut registry = HandleRegistry::new();
        let handle = registry.create_effect(0);
        assert!(registry.is_valid_effect(handle));
        // 保留條件：(current_frame - created_frame) < max_lifetime_frames
        // 300 - 0 = 300，300 ≮ 300 → 移除
        registry.sweep_expired(300);
        assert!(!registry.is_valid_effect(handle));
    }

    #[test]
    fn test_handle_not_expired_yet() {
        let mut registry = HandleRegistry::new();
        let handle = registry.create_effect(0);
        // 299 - 0 = 299，299 < 300 → 保留
        registry.sweep_expired(299);
        assert!(registry.is_valid_effect(handle));
    }

    #[test]
    fn test_sweep_removes_invalid() {
        let mut registry = HandleRegistry::new();
        let handle = registry.create_effect(0);
        registry.invalidate_effect(handle);
        // valid=false → sweep 一律移除（無論幀數）
        registry.sweep_expired(0);
        // sweep 後 entry 已從 BTreeMap 移除，is_valid 查無 key → false
        assert!(!registry.is_valid_effect(handle));
    }

    // --- Task 10 補充測試 ---

    #[test]
    fn test_mixed_effect_sound_ids() {
        let mut registry = HandleRegistry::new();
        let e1 = registry.create_effect(0);
        let s1 = registry.create_sound(0);
        let e2 = registry.create_effect(0);
        // ID 應該是 0, 1, 2 — 唯一遞增（共享 next_id）
        assert_eq!(e1.0, 0);
        assert_eq!(s1.0, 1);
        assert_eq!(e2.0, 2);
        // 跨 map 驗證：effect map 有 0, 2；sound map 有 1
        assert!(registry.is_valid_effect(e1));
        assert!(!registry.is_valid_effect(EffectHandle(1))); // 1 在 sounds map
        assert!(registry.is_valid_sound(s1));
        assert!(!registry.is_valid_sound(SoundHandle(0))); // 0 在 effects map
    }

    #[test]
    fn test_sweep_only_effects() {
        let mut registry = HandleRegistry::new();
        let effect = registry.create_effect(0);
        let sound = registry.create_sound(100);
        // frame 300: effect 過期（300 - 0 = 300，300 ≮ 300 → 移除），
        //            sound 未過期（300 - 100 = 200，200 < 300 → 保留）
        registry.sweep_expired(300);
        assert!(!registry.is_valid_effect(effect));
        assert!(registry.is_valid_sound(sound));
        // 驗證 map 大小
        assert_eq!(registry.effect_count(), 0);
        assert_eq!(registry.sound_count(), 1);
    }
}
