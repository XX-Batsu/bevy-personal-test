//! WASM 記憶體區域監控模組。
//!
//! 提供七個 WASM 線性記憶體區域的使用量追蹤、邊界保護與定時報告。
//! 監控頻率：每 60 幀（使用 frame counter，禁止 `std::time`）。
//! 閾值策略：90% → warn，100% → error + panic。
//!
//! 對應設計文件：`docs/design/architecture/13-memory-layout/wasm-regions.md`

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::fixed_update::FixedTickCounter;

// ── 常數 ─────────────────────────────────────────────────────────────────

/// 總記憶體上限（512 MB）
pub const TOTAL_MEMORY_CAP: usize = 512 * 1024 * 1024;

// ── MemoryRegion ─────────────────────────────────────────────────────────

/// 七個 WASM 記憶體區域識別符。
///
/// 對應 `13-memory-layout/wasm-regions.md` §MemoryRegion 枚舉。
/// 不參與 FFI 或跨版本序列化，無需 `#[repr(C)]`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemoryRegion {
    /// ECS 堆積區（0x00000000 起，256 MB）
    EcsHeap,
    /// VM 執行時區（0x10000000 起，64 MB）
    VmRuntime,
    /// VM 堆疊區（0x14000000 起，4 MB）
    VmStack,
    /// ECS 鏡像區（0x14400000 起，16 MB）
    EcsMirror,
    /// Shadow VM 驗證區（0x15400000 起，32 MB）
    ShadowValidation,
    /// L1 執行快取區（0x17400000 起，8 MB）
    L1Cache,
    /// 可成長的執行時堆積區（0x17C00000 起，132 MB）
    RuntimeHeap,
}

/// 所有區域，依序列出。
const ALL_REGIONS: [MemoryRegion; 7] = [
    MemoryRegion::EcsHeap,
    MemoryRegion::VmRuntime,
    MemoryRegion::VmStack,
    MemoryRegion::EcsMirror,
    MemoryRegion::ShadowValidation,
    MemoryRegion::L1Cache,
    MemoryRegion::RuntimeHeap,
];

impl MemoryRegion {
    /// 區域大小上限（bytes）。
    pub const fn max_bytes(self) -> usize {
        match self {
            Self::EcsHeap => 256 * 1024 * 1024,
            Self::VmRuntime => 64 * 1024 * 1024,
            Self::VmStack => 4 * 1024 * 1024,
            Self::EcsMirror => 16 * 1024 * 1024,
            Self::ShadowValidation => 32 * 1024 * 1024,
            Self::L1Cache => 8 * 1024 * 1024,
            Self::RuntimeHeap => 132 * 1024 * 1024,
        }
    }

    /// 區域線性記憶體起始偏移（bytes）。
    pub const fn base_offset(self) -> usize {
        match self {
            Self::EcsHeap => 0x0000_0000,
            Self::VmRuntime => 0x1000_0000,
            Self::VmStack => 0x1400_0000,
            Self::EcsMirror => 0x1440_0000,
            Self::ShadowValidation => 0x1540_0000,
            Self::L1Cache => 0x1740_0000,
            Self::RuntimeHeap => 0x17C0_0000,
        }
    }

    /// 是否允許成長（僅 RuntimeHeap）。
    pub const fn growable(self) -> bool {
        matches!(self, Self::RuntimeHeap)
    }
}

// ── RegionReport ─────────────────────────────────────────────────────────

/// 記憶體使用量監控報告（每 60 幀產生一次）。
///
/// 純內部監控型別，不參與 FFI 或跨版本序列化。
/// 對應 `wasm-regions.md` §RegionReport（單筆記錄結構）。
#[derive(Debug, Clone)]
pub struct RegionReport {
    /// 區域識別符。
    pub region: MemoryRegion,
    /// 目前使用量（bytes）。
    pub used_bytes: usize,
    /// 區域容量上限（bytes）= `region.max_bytes()`。
    pub limit_bytes: usize,
    /// 數值為精確測量（`false` = 估算值）。
    /// EcsHeap、RuntimeHeap 為估算（`is_exact = false`）。
    pub is_exact: bool,
}

// ── MemoryError ──────────────────────────────────────────────────────────

/// 記憶體錯誤型別。
///
/// 對應 `wasm-regions.md` §MemoryError。
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// 區域溢出。
    #[error("區域 {region:?} 溢出：申請 {requested} bytes，當前 {current}/{limit}")]
    RegionOverflow {
        region: MemoryRegion,
        requested: usize,
        current: usize,
        limit: usize,
    },
    /// 記憶體使用量超過總上限。
    #[error("記憶體使用量超過總上限 512 MB")]
    TotalCapExceeded,
    /// 整數溢出。
    #[error("整數溢出")]
    Overflow,
    /// WASM memory.grow 失敗。
    #[error("WASM memory.grow 失敗：申請 {pages} 頁")]
    GrowFailed { pages: u32 },
}

// ── RegionGuard ──────────────────────────────────────────────────────────

/// 區域邊界保護器。
///
/// 追蹤每個區域的當前使用量，並強制執行上限。
/// `usage` 使用 `BTreeMap`（禁止 HashMap，README.md Determinism Rules）。
#[derive(Default)]
pub struct RegionGuard {
    usage: BTreeMap<MemoryRegion, usize>,
}

impl RegionGuard {
    /// 向指定區域申請 `bytes`，超出上限時回傳 `Err(MemoryError)`。
    pub fn allocate(&mut self, region: MemoryRegion, bytes: usize) -> Result<(), MemoryError> {
        let current = self.usage.entry(region).or_insert(0);
        let new_total = current.checked_add(bytes).ok_or(MemoryError::Overflow)?;
        if new_total > region.max_bytes() {
            return Err(MemoryError::RegionOverflow {
                region,
                requested: bytes,
                current: *current,
                limit: region.max_bytes(),
            });
        }
        *current = new_total;
        Ok(())
    }

    /// 釋放指定區域的 `bytes`（`saturating_sub`，不會負數）。
    pub fn deallocate(&mut self, region: MemoryRegion, bytes: usize) {
        let current = self.usage.entry(region).or_insert(0);
        *current = current.saturating_sub(bytes);
    }

    /// 取得指定區域當前使用量。
    pub fn usage_bytes(&self, region: MemoryRegion) -> usize {
        self.usage.get(&region).copied().unwrap_or(0)
    }

    /// 計算所有區域總使用量。
    pub fn total_usage_bytes(&self) -> usize {
        self.usage.values().sum()
    }

    /// 是否超過總量警告閾值（90%，即 460.8 MB）。
    pub fn is_warn_threshold(&self) -> bool {
        // 先除再乘，避免 WASM 32-bit usize 乘法溢位（512MB * 90 > u32::MAX）
        self.total_usage_bytes() > TOTAL_MEMORY_CAP / 100 * 90
    }
}

// ── MemoryMonitor ────────────────────────────────────────────────────────

/// 記憶體監控器（Bevy Resource）。
///
/// 使用 frame counter 觸發（禁止使用 `std::time`）。
/// 對應 `wasm-regions.md` §MemoryMonitor。
///
/// 內含 [`RegionGuard`] 進行邊界保護（Phase 15 精確替代）。
#[derive(Resource, Default)]
pub struct MemoryMonitor {
    /// 區域邊界保護器。
    pub(crate) guard: RegionGuard,
    /// 上一次報告的 frame（`None` = 尚未報告過）。
    last_report_frame: Option<u64>,
}

impl MemoryMonitor {
    /// 每幀呼叫，每 60 幀產生一次報告。
    ///
    /// 使用 frame counter 差值判斷（禁止 `std::time`）。
    /// 首次呼叫立即觸發報告。
    pub fn tick(&mut self, frame: u64) -> Option<Vec<RegionReport>> {
        match self.last_report_frame {
            None => {
                // 首次呼叫：立即產生報告
                self.last_report_frame = Some(frame);
                Some(self.build_report())
            }
            Some(last) if frame.saturating_sub(last) >= 60 => {
                // 已過 60 幀間隔：產生報告
                self.last_report_frame = Some(frame);
                Some(self.build_report())
            }
            _ => None,
        }
    }

    /// 產生所有區域的報告。
    fn build_report(&self) -> Vec<RegionReport> {
        ALL_REGIONS
            .iter()
            .map(|&region| RegionReport {
                region,
                used_bytes: self.guard.usage_bytes(region),
                limit_bytes: region.max_bytes(),
                is_exact: !matches!(region, MemoryRegion::EcsHeap | MemoryRegion::RuntimeHeap),
            })
            .collect()
    }
}

// ── monitor_memory system ────────────────────────────────────────────────

/// 記憶體監控系統（排程於 `Last` schedule）。
///
/// 透過 [`MemoryMonitor::tick`] 每 60 幀產生報告，
/// 並依據閾值發出警告或觸發 panic。
fn monitor_memory(mut monitor: ResMut<MemoryMonitor>, tick: Res<FixedTickCounter>) {
    if let Some(_reports) = monitor.tick(tick.count) {
        let total = monitor.guard.total_usage_bytes();
        if total > TOTAL_MEMORY_CAP {
            tracing::error!(total_bytes = total, "記憶體已超過 512MB 上限");
            panic!("WASM 記憶體超過 512MB 上限");
        } else if monitor.guard.is_warn_threshold() {
            let pct = total * 100 / TOTAL_MEMORY_CAP;
            tracing::warn!(pct, total_bytes = total, "記憶體使用率達 {}%", pct);
        }
    }
}

// ── MemoryMonitorPlugin ──────────────────────────────────────────────────

/// 記憶體監控 Plugin。
///
/// 初始化 [`MemoryMonitor`] Resource 並將 [`monitor_memory`] 系統
/// 排程至 `Last` schedule。
pub struct MemoryMonitorPlugin;

impl Plugin for MemoryMonitorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MemoryMonitor>();
        app.add_systems(Last, monitor_memory);
    }
}

// ── 測試 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 1. MemoryRegion::max_bytes() ─────────────────────────────────────

    #[test]
    fn test_region_max_bytes_ecs_heap() {
        assert_eq!(
            MemoryRegion::EcsHeap.max_bytes(),
            256 * 1024 * 1024,
            "EcsHeap 應為 256 MB"
        );
    }

    // ── 2. 所有區域 max_bytes() 之和 ────────────────────────────────────

    #[test]
    fn test_region_all_fixed_sum() {
        let sum: usize = ALL_REGIONS.iter().map(|r| r.max_bytes()).sum();
        assert_eq!(
            sum, TOTAL_MEMORY_CAP,
            "所有區域 max_bytes() 之和應等於 TOTAL_MEMORY_CAP (512 MB)"
        );
    }

    // ── 3. 區域位址不重疊 ────────────────────────────────────────────────

    #[test]
    fn test_region_base_offsets_no_overlap() {
        let mut ranges: Vec<(usize, usize, MemoryRegion)> = ALL_REGIONS
            .iter()
            .map(|&r| (r.base_offset(), r.base_offset() + r.max_bytes(), r))
            .collect();
        ranges.sort_by_key(|&(start, _, _)| start);

        for window in ranges.windows(2) {
            let (_, end_a, region_a) = window[0];
            let (start_b, _, region_b) = window[1];
            assert!(
                end_a <= start_b,
                "區域 {region_a:?} [..{end_a:#x}) 與 {region_b:?} [{start_b:#x}..) 重疊"
            );
        }
    }

    // ── 4. 僅 RuntimeHeap 可成長 ────────────────────────────────────────

    #[test]
    fn test_region_growable_only_runtime_heap() {
        for &region in &ALL_REGIONS {
            if matches!(region, MemoryRegion::RuntimeHeap) {
                assert!(region.growable(), "RuntimeHeap 應可成長");
            } else {
                assert!(!region.growable(), "{region:?} 不應可成長");
            }
        }
    }

    // ── 5. RegionGuard::allocate() 正常路徑 ─────────────────────────────

    #[test]
    fn test_guard_allocate_ok() {
        let mut guard = RegionGuard::default();
        let one_mb = 1024 * 1024;
        let result = guard.allocate(MemoryRegion::EcsMirror, one_mb);
        assert!(result.is_ok(), "申請 1 MB EcsMirror 應成功");
        assert_eq!(
            guard.usage_bytes(MemoryRegion::EcsMirror),
            one_mb,
            "使用量應為 1 MB"
        );
    }

    // ── 6. RegionGuard::allocate() 溢出 ─────────────────────────────────

    #[test]
    fn test_guard_allocate_overflow() {
        let mut guard = RegionGuard::default();
        let seventeen_mb = 17 * 1024 * 1024;
        let result = guard.allocate(MemoryRegion::EcsMirror, seventeen_mb);
        assert!(result.is_err(), "申請 17 MB EcsMirror（上限 16 MB）應失敗");
        match result.unwrap_err() {
            MemoryError::RegionOverflow { region, .. } => {
                assert_eq!(region, MemoryRegion::EcsMirror);
            }
            other => panic!("預期 RegionOverflow，實際: {other:?}"),
        }
    }

    // ── 7. RegionGuard::allocate() 整數溢出 ─────────────────────────────

    #[test]
    fn test_guard_allocate_integer_overflow() {
        let mut guard = RegionGuard::default();
        // 先分配 1 byte 使 checked_add(usize::MAX) 溢出
        guard
            .allocate(MemoryRegion::EcsHeap, 1)
            .expect("1 byte 分配應成功");
        let result = guard.allocate(MemoryRegion::EcsHeap, usize::MAX);
        assert!(result.is_err(), "usize::MAX 應導致整數溢出");
        match result.unwrap_err() {
            MemoryError::Overflow => {}
            other => panic!("預期 Overflow，實際: {other:?}"),
        }
    }

    // ── 8. RegionGuard::deallocate() 不會下溢 ──────────────────────────

    #[test]
    fn test_guard_deallocate_no_underflow() {
        let mut guard = RegionGuard::default();
        let one_mb = 1024 * 1024;
        guard
            .allocate(MemoryRegion::VmStack, one_mb)
            .expect("分配應成功");
        // 釋放比使用量更多的量
        guard.deallocate(MemoryRegion::VmStack, 2 * one_mb);
        assert_eq!(
            guard.usage_bytes(MemoryRegion::VmStack),
            0,
            "釋放超過使用量後應為 0（saturating_sub）"
        );
    }

    // ── 9. RegionGuard::total_usage_bytes() ──────────────────────────────

    #[test]
    fn test_guard_total_usage() {
        let mut guard = RegionGuard::default();
        let one_mb = 1024 * 1024;
        for &region in &ALL_REGIONS {
            guard
                .allocate(region, one_mb)
                .expect("每區域 1 MB 分配應成功");
        }
        assert_eq!(guard.total_usage_bytes(), 7 * one_mb, "總使用量應為 7 MB");
    }

    // ── 10. RegionGuard::is_warn_threshold() ─────────────────────────────

    #[test]
    fn test_guard_warn_threshold() {
        let mut guard = RegionGuard::default();
        // 90% of 512 MB = 460.8 MB；分配 461 MB 超過閾值
        // EcsHeap 最大 256 MB，RuntimeHeap 最大 132 MB，VmRuntime 最大 64 MB
        // 256 + 132 + 64 = 452 MB，再加 ShadowValidation 的 9 MB = 461 MB
        guard
            .allocate(MemoryRegion::EcsHeap, 256 * 1024 * 1024)
            .expect("EcsHeap 分配應成功");
        guard
            .allocate(MemoryRegion::RuntimeHeap, 132 * 1024 * 1024)
            .expect("RuntimeHeap 分配應成功");
        guard
            .allocate(MemoryRegion::VmRuntime, 64 * 1024 * 1024)
            .expect("VmRuntime 分配應成功");
        guard
            .allocate(MemoryRegion::ShadowValidation, 9 * 1024 * 1024)
            .expect("ShadowValidation 9 MB 分配應成功");
        // 總量 = 461 MB > 460.8 MB
        assert!(
            guard.is_warn_threshold(),
            "461 MB > 90% of 512 MB，應觸發警告閾值"
        );
    }

    // ── 11. MemoryMonitor::tick() 首次與間隔不足 ─────────────────────────

    #[test]
    fn test_monitor_tick_before_interval() {
        let mut monitor = MemoryMonitor::default();
        // 首次呼叫 tick(0) → Some（首次報告）
        let first = monitor.tick(0);
        assert!(first.is_some(), "tick(0) 應為首次報告（Some）");
        // 間隔不足 60 幀
        let second = monitor.tick(59);
        assert!(second.is_none(), "tick(59) 應為 None（間隔不足 60 幀）");
    }

    // ── 12. MemoryMonitor::tick() 滿 60 幀觸發 ──────────────────────────

    #[test]
    fn test_monitor_tick_at_interval() {
        let mut monitor = MemoryMonitor::default();
        // 首次報告
        let first = monitor.tick(0);
        assert!(first.is_some(), "tick(0) 首次報告");
        // 間隔 60 幀
        let reports = monitor.tick(60);
        assert!(reports.is_some(), "tick(60) 應產生報告");
        let reports = reports.unwrap();
        assert_eq!(reports.len(), 7, "報告應含 7 個區域");
    }

    // ── 13. RegionReport::is_exact 分類 ──────────────────────────────────

    #[test]
    fn test_monitor_report_is_exact_classification() {
        let mut monitor = MemoryMonitor::default();
        let reports = monitor.tick(0).expect("首次報告應存在");
        for report in &reports {
            match report.region {
                MemoryRegion::EcsHeap | MemoryRegion::RuntimeHeap => {
                    assert!(
                        !report.is_exact,
                        "{:?} 的 is_exact 應為 false（估算值）",
                        report.region
                    );
                }
                _ => {
                    assert!(
                        report.is_exact,
                        "{:?} 的 is_exact 應為 true（精確值）",
                        report.region
                    );
                }
            }
        }
    }

    // ── 14. RegionReport::limit_bytes 一致性 ─────────────────────────────

    #[test]
    fn test_monitor_report_limit_bytes_matches_max_bytes() {
        let mut monitor = MemoryMonitor::default();
        let reports = monitor.tick(0).expect("首次報告應存在");
        for report in &reports {
            assert_eq!(
                report.limit_bytes,
                report.region.max_bytes(),
                "{:?} 的 limit_bytes 應等於 max_bytes()",
                report.region
            );
        }
    }
}
