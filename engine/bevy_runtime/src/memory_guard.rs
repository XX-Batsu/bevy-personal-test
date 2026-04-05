//! WASM 記憶體 Region 邊界守衛模組（Phase 15）。
//!
//! 提供 [`RegionGuard`] 做為輕量邊界檢查器：
//! - [`RegionGuard::check_allocation`]：預先檢查，不改動狀態
//! - [`RegionGuard::record_allocation`] / [`RegionGuard::record_deallocation`]：更新狀態
//! - [`RegionGuard::usage_of`] / [`RegionGuard::total_usage`]：查詢用量
//!
//! **確定性保證**：使用 `[usize; 7]` 固定陣列，無 HashMap/HashSet。
//! **WASM 限制**：無 std::time、無系統 RNG、無原生 float。
//!
//! 對應設計文件：`docs/design/architecture/13-memory-layout/`

use std::fmt;

// ── MemoryRegion ─────────────────────────────────────────────────────────

/// 七個 WASM 記憶體區域識別符（Phase 15 守衛用，含 `#[repr(u8)]` 索引保證）。
///
/// 各區域上限之和等於 [`TOTAL_MEMORY_CAP`]（512 MB）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MemoryRegion {
    /// ECS 堆積區（256 MB）
    EcsHeap = 0,
    /// VM 執行時區（64 MB）
    VmRuntime = 1,
    /// VM 堆疊區（4 MB）
    VmStack = 2,
    /// ECS 鏡像區（16 MB）
    EcsMirror = 3,
    /// Shadow VM 驗證區（32 MB）
    ShadowValidation = 4,
    /// L1 執行快取區（8 MB）
    L1Cache = 5,
    /// 可成長執行時堆積區（132 MB）
    RuntimeHeap = 6,
}

impl MemoryRegion {
    /// 區域大小上限（bytes）。
    ///
    /// 所有區域上限之和恰好等於 [`TOTAL_MEMORY_CAP`]（512 MB）。
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
}

// ── 總量上限 ──────────────────────────────────────────────────────────────

/// WASM 線性記憶體總量上限（512 MB）。
///
/// 等於所有 [`MemoryRegion`] 上限之和。
pub const TOTAL_MEMORY_CAP: usize = 512 * 1024 * 1024;

/// WASM page 大小（64 KB = 65,536 bytes），WebAssembly MVP 規格。
pub const WASM_PAGE_SIZE: usize = 65_536;

// ── MemoryError ──────────────────────────────────────────────────────────

/// 記憶體邊界守衛錯誤型別。
#[derive(Debug)]
pub enum MemoryError {
    /// 區域用量超出上限。
    RegionOverflow {
        region: MemoryRegion,
        requested: usize,
        current: usize,
        limit: usize,
    },
    /// 所有區域加總超出 WASM 總量上限。
    TotalCapExceeded { cap: usize, total: usize },
    /// Heap 成長失敗。
    GrowthFailed {
        requested_pages: usize,
        reason: String,
    },
    /// 整數溢位。
    Overflow,
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RegionOverflow {
                region,
                requested,
                current,
                limit,
            } => write!(
                f,
                "記憶體區段 {:?} 溢位：當前 {} bytes / 上限 {} bytes，請求 {} bytes",
                region, current, limit, requested
            ),
            Self::TotalCapExceeded { cap, total } => write!(
                f,
                "WASM 記憶體總量超出上限：上限 {} bytes，目前總計 {} bytes",
                cap, total
            ),
            Self::GrowthFailed {
                requested_pages,
                reason,
            } => write!(
                f,
                "Heap 成長失敗：請求 {} 頁，原因：{}",
                requested_pages, reason
            ),
            Self::Overflow => write!(f, "記憶體用量計算整數溢位"),
        }
    }
}

impl std::error::Error for MemoryError {}

// ── RegionGuard ──────────────────────────────────────────────────────────

/// WASM 記憶體 Region 邊界守衛。
///
/// 使用 `[usize; 7]` 固定陣列追蹤各區域用量（禁止 HashMap，確定性保證）。
/// 索引與 [`MemoryRegion`] 的 `#[repr(u8)]` 判別值一一對應。
///
/// # 使用方式
///
/// ```ignore
/// let mut guard = RegionGuard::new();
/// guard.check_allocation(MemoryRegion::EcsHeap, 100 * 1024 * 1024)?;
/// guard.record_allocation(MemoryRegion::EcsHeap, 100 * 1024 * 1024);
/// ```
pub struct RegionGuard {
    usage: [usize; 7],
}

impl RegionGuard {
    /// 建立新的 [`RegionGuard`]，所有區域用量初始為 0。
    pub fn new() -> Self {
        Self { usage: [0; 7] }
    }

    /// 預先檢查是否可以在 `region` 中分配 `additional_bytes`。
    ///
    /// 不改動任何狀態。檢查順序（固定，不可更改）：
    /// 1. `current + additional_bytes` 整數溢位偵測（`Overflow`）
    /// 2. `total + additional_bytes` 整數溢位偵測（`Overflow`）
    /// 3. 全域 512 MB 上限（`TotalCapExceeded`，優先於區域上限）
    /// 4. 區域上限（`RegionOverflow`）
    ///
    /// 步驟 3 在步驟 4 之前，確保總量超出時優先回傳 `TotalCapExceeded`。
    pub fn check_allocation(
        &self,
        region: MemoryRegion,
        additional_bytes: usize,
    ) -> Result<(), MemoryError> {
        let current = self.usage[region as usize];

        // 步驟 1：偵測 current + additional_bytes 溢位
        let new_usage = current
            .checked_add(additional_bytes)
            .ok_or(MemoryError::Overflow)?;

        // 步驟 2：總量溢位偵測
        let total_before: usize = self.usage.iter().sum();
        let new_total = total_before
            .checked_add(additional_bytes)
            .ok_or(MemoryError::Overflow)?;

        // 步驟 3：全域 512 MB 上限（優先於區域上限）
        if new_total > TOTAL_MEMORY_CAP {
            return Err(MemoryError::TotalCapExceeded {
                cap: TOTAL_MEMORY_CAP,
                total: new_total,
            });
        }

        // 步驟 4：區域上限
        if new_usage > region.max_bytes() {
            return Err(MemoryError::RegionOverflow {
                region,
                requested: additional_bytes,
                current,
                limit: region.max_bytes(),
            });
        }

        Ok(())
    }

    /// 記錄 `region` 中已分配 `bytes`（不做邊界檢查，請先呼叫 [`check_allocation`]）。
    ///
    /// 使用 `saturating_add` 避免溢位 panic。
    ///
    /// [`check_allocation`]: RegionGuard::check_allocation
    pub fn record_allocation(&mut self, region: MemoryRegion, bytes: usize) {
        self.usage[region as usize] = self.usage[region as usize].saturating_add(bytes);
    }

    /// 記錄 `region` 中已釋放 `bytes`（`saturating_sub`，不會產生負數）。
    pub fn record_deallocation(&mut self, region: MemoryRegion, bytes: usize) {
        self.usage[region as usize] = self.usage[region as usize].saturating_sub(bytes);
    }

    /// 取得 `region` 目前用量（bytes）。
    pub fn usage_of(&self, region: MemoryRegion) -> usize {
        self.usage[region as usize]
    }

    /// 取得所有區域總用量（bytes）。
    pub fn total_usage(&self) -> usize {
        self.usage.iter().sum()
    }

    /// 嘗試增長 Runtime Heap
    ///
    /// `additional_bytes` = 0 時為 no-op，直接回傳 `Ok(())`。
    /// Native build：只做邊界檢查並更新計數。
    /// WASM build：先邊界檢查，再呼叫 wasm_grow_pages，成功後更新計數。
    ///
    /// 三步流程：check_allocation → wasm_grow_pages（僅 wasm32）→ record_allocation
    pub fn try_grow_heap(&mut self, additional_bytes: usize) -> Result<(), MemoryError> {
        if additional_bytes == 0 {
            return Ok(());
        }

        // Step 1: 邊界檢查（不修改 usage）
        self.check_allocation(MemoryRegion::RuntimeHeap, additional_bytes)?;

        // Step 2: WASM 實際成長（僅 wasm32 target）
        #[cfg(target_arch = "wasm32")]
        {
            let pages = (additional_bytes + WASM_PAGE_SIZE - 1) / WASM_PAGE_SIZE;
            wasm_grow_pages(pages)?;
        }

        // Step 3: 更新計數
        self.record_allocation(MemoryRegion::RuntimeHeap, additional_bytes);
        Ok(())
    }
}

impl Default for RegionGuard {
    fn default() -> Self {
        Self::new()
    }
}

// ── WASM memory.grow 輔助 ────────────────────────────────────────────────

/// WASM memory.grow 輔助函式（僅 wasm32 target）
#[cfg(target_arch = "wasm32")]
fn wasm_grow_pages(pages: usize) -> Result<(), MemoryError> {
    use wasm_bindgen::JsCast;
    let memory = wasm_bindgen::memory()
        .dyn_into::<js_sys::WebAssembly::Memory>()
        .map_err(|_| MemoryError::GrowthFailed {
            requested_pages: pages,
            reason: "無法取得 WASM Memory 物件".to_string(),
        })?;
    // 安全：pages ≤ 2048（check_allocation 已保證 additional_bytes ≤ 132 MB）
    let _prev = memory.grow(pages as u32);
    Ok(())
}

// ── 單元測試 ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_regions_max_bytes() {
        assert_eq!(MemoryRegion::EcsHeap.max_bytes(), 256 * 1024 * 1024);
        assert_eq!(MemoryRegion::VmRuntime.max_bytes(), 64 * 1024 * 1024);
        assert_eq!(MemoryRegion::VmStack.max_bytes(), 4 * 1024 * 1024);
        assert_eq!(MemoryRegion::EcsMirror.max_bytes(), 16 * 1024 * 1024);
        assert_eq!(MemoryRegion::ShadowValidation.max_bytes(), 32 * 1024 * 1024);
        assert_eq!(MemoryRegion::L1Cache.max_bytes(), 8 * 1024 * 1024);
        assert_eq!(MemoryRegion::RuntimeHeap.max_bytes(), 132 * 1024 * 1024);
        let sum: usize = [
            MemoryRegion::EcsHeap,
            MemoryRegion::VmRuntime,
            MemoryRegion::VmStack,
            MemoryRegion::EcsMirror,
            MemoryRegion::ShadowValidation,
            MemoryRegion::L1Cache,
            MemoryRegion::RuntimeHeap,
        ]
        .iter()
        .map(|r| r.max_bytes())
        .sum();
        assert_eq!(sum, TOTAL_MEMORY_CAP);
    }

    #[test]
    fn test_region_within_budget() {
        let guard = RegionGuard::new();
        assert!(guard
            .check_allocation(MemoryRegion::EcsHeap, 100 * 1024 * 1024)
            .is_ok());
    }

    #[test]
    fn test_region_exact_boundary() {
        let guard = RegionGuard::new();
        assert!(guard
            .check_allocation(MemoryRegion::EcsHeap, 256 * 1024 * 1024)
            .is_ok());
    }

    #[test]
    fn test_region_overflow() {
        let guard = RegionGuard::new();
        let result = guard.check_allocation(MemoryRegion::EcsHeap, 257 * 1024 * 1024);
        match result {
            Err(MemoryError::RegionOverflow {
                region,
                requested,
                current,
                limit,
            }) => {
                assert_eq!(region, MemoryRegion::EcsHeap);
                assert_eq!(requested, 257 * 1024 * 1024);
                assert_eq!(current, 0);
                assert_eq!(limit, 256 * 1024 * 1024);
            }
            other => panic!("預期 RegionOverflow，實際: {:?}", other),
        }
    }

    #[test]
    fn test_region_overflow_accumulated() {
        let mut guard = RegionGuard::new();
        guard.record_allocation(MemoryRegion::EcsHeap, 200 * 1024 * 1024);
        let result = guard.check_allocation(MemoryRegion::EcsHeap, 100 * 1024 * 1024);
        match result {
            Err(MemoryError::RegionOverflow { current, limit, .. }) => {
                assert_eq!(current, 200 * 1024 * 1024);
                assert_eq!(limit, 256 * 1024 * 1024);
            }
            other => panic!("預期 RegionOverflow，實際: {:?}", other),
        }
    }

    #[test]
    fn test_vm_runtime_cap_64mb() {
        let guard = RegionGuard::new();
        let result = guard.check_allocation(MemoryRegion::VmRuntime, 65 * 1024 * 1024);
        assert!(
            matches!(result, Err(MemoryError::RegionOverflow { limit, .. }) if limit == 64 * 1024 * 1024)
        );
    }

    #[test]
    fn test_ecs_mirror_cap_16mb() {
        let guard = RegionGuard::new();
        let result = guard.check_allocation(MemoryRegion::EcsMirror, 17 * 1024 * 1024);
        assert!(
            matches!(result, Err(MemoryError::RegionOverflow { limit, .. }) if limit == 16 * 1024 * 1024)
        );
    }

    #[test]
    fn test_l1_cache_cap_8mb() {
        let guard = RegionGuard::new();
        let result = guard.check_allocation(MemoryRegion::L1Cache, 9 * 1024 * 1024);
        assert!(
            matches!(result, Err(MemoryError::RegionOverflow { limit, .. }) if limit == 8 * 1024 * 1024)
        );
    }

    #[test]
    fn test_total_cap_512mb() {
        let mut guard = RegionGuard::new();
        guard.record_allocation(MemoryRegion::EcsHeap, 250 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::VmRuntime, 60 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::EcsMirror, 15 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::ShadowValidation, 30 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::L1Cache, 7 * 1024 * 1024);
        let result = guard.check_allocation(MemoryRegion::RuntimeHeap, 200 * 1024 * 1024);
        match result {
            Err(MemoryError::TotalCapExceeded { cap, total }) => {
                assert_eq!(cap, TOTAL_MEMORY_CAP);
                assert_eq!(total, (362 + 200) * 1024 * 1024);
            }
            other => panic!("預期 TotalCapExceeded，實際: {:?}", other),
        }
    }

    #[test]
    fn test_check_allocation_overflow() {
        let mut guard = RegionGuard::new();
        guard.record_allocation(MemoryRegion::EcsHeap, 1);
        let result = guard.check_allocation(MemoryRegion::EcsHeap, usize::MAX);
        assert!(matches!(result, Err(MemoryError::Overflow)));
    }

    #[test]
    fn test_record_deallocation_then_reallocate() {
        let mut guard = RegionGuard::new();
        guard.record_allocation(MemoryRegion::EcsHeap, 50 * 1024 * 1024);
        assert_eq!(guard.usage_of(MemoryRegion::EcsHeap), 50 * 1024 * 1024);
        guard.record_deallocation(MemoryRegion::EcsHeap, 50 * 1024 * 1024);
        assert_eq!(guard.usage_of(MemoryRegion::EcsHeap), 0);
        assert!(guard
            .check_allocation(MemoryRegion::EcsHeap, 50 * 1024 * 1024)
            .is_ok());
    }

    #[test]
    fn test_record_deallocation_saturating_sub() {
        let mut guard = RegionGuard::new();
        guard.record_deallocation(MemoryRegion::EcsHeap, 10 * 1024 * 1024);
        assert_eq!(guard.usage_of(MemoryRegion::EcsHeap), 0);
    }

    #[test]
    fn test_multi_region_independent_tracking() {
        let mut guard = RegionGuard::new();
        guard.record_allocation(MemoryRegion::EcsHeap, 100 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::VmRuntime, 30 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::EcsMirror, 10 * 1024 * 1024);
        assert_eq!(guard.usage_of(MemoryRegion::EcsHeap), 100 * 1024 * 1024);
        assert_eq!(guard.usage_of(MemoryRegion::VmRuntime), 30 * 1024 * 1024);
        assert_eq!(guard.usage_of(MemoryRegion::EcsMirror), 10 * 1024 * 1024);
        assert_eq!(guard.usage_of(MemoryRegion::VmStack), 0);
        assert_eq!(guard.usage_of(MemoryRegion::L1Cache), 0);
        assert_eq!(guard.total_usage(), 140 * 1024 * 1024);
    }
}

#[cfg(test)]
mod heap_growth_tests {
    use super::*;

    #[test]
    fn test_wasm_page_size_constant() {
        assert_eq!(WASM_PAGE_SIZE, 65_536);
    }

    #[test]
    fn test_grow_zero_bytes_is_noop() {
        let mut guard = RegionGuard::new();
        assert!(guard.try_grow_heap(0).is_ok());
        assert_eq!(guard.usage_of(MemoryRegion::RuntimeHeap), 0);
    }

    #[test]
    fn test_grow_within_total_cap() {
        let mut guard = RegionGuard::new();
        assert!(guard.try_grow_heap(50 * 1024 * 1024).is_ok());
        assert_eq!(guard.usage_of(MemoryRegion::RuntimeHeap), 50 * 1024 * 1024);
    }

    #[test]
    fn test_grow_cumulative() {
        let mut guard = RegionGuard::new();
        assert!(guard.try_grow_heap(30 * 1024 * 1024).is_ok());
        assert!(guard.try_grow_heap(20 * 1024 * 1024).is_ok());
        assert_eq!(guard.usage_of(MemoryRegion::RuntimeHeap), 50 * 1024 * 1024);
    }

    #[test]
    fn test_grow_exact_region_limit() {
        let mut guard = RegionGuard::new();
        assert!(guard.try_grow_heap(132 * 1024 * 1024).is_ok());
        assert_eq!(guard.usage_of(MemoryRegion::RuntimeHeap), 132 * 1024 * 1024);
        // 再成長 1 byte → RegionOverflow（已到達 region limit）
        let result = guard.try_grow_heap(1);
        assert!(matches!(result, Err(MemoryError::RegionOverflow { .. })));
    }

    #[test]
    fn test_grow_exceeds_total_cap() {
        let mut guard = RegionGuard::new();
        // 透過 record_allocation 設定其他 region 合計 390MB
        guard.record_allocation(MemoryRegion::EcsHeap, 256 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::VmRuntime, 64 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::VmStack, 4 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::EcsMirror, 16 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::ShadowValidation, 32 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::L1Cache, 18 * 1024 * 1024); // 超過 8MB 上限，測試用
                                                                          // 總計 390MB，成長 130MB → 520MB > 512MB → TotalCapExceeded
        let result = guard.try_grow_heap(130 * 1024 * 1024);
        assert!(matches!(result, Err(MemoryError::TotalCapExceeded { .. })));
    }

    #[test]
    fn test_grow_exceeds_total_cap_within_region_limit() {
        let mut guard = RegionGuard::new();
        // 固定 region 380MB + RuntimeHeap 132MB = 512MB（恰好等於上限）
        guard.record_allocation(MemoryRegion::EcsHeap, 256 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::VmRuntime, 64 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::EcsMirror, 16 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::ShadowValidation, 32 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::L1Cache, 8 * 1024 * 1024);
        guard.record_allocation(MemoryRegion::VmStack, 4 * 1024 * 1024);
        // 總計 380MB，成長 132MB → 512MB = TOTAL_MEMORY_CAP → Ok
        assert!(guard.try_grow_heap(132 * 1024 * 1024).is_ok());
    }

    #[test]
    fn test_grow_step_1mb_pages() {
        let mut guard = RegionGuard::new();
        assert!(guard.try_grow_heap(1024 * 1024).is_ok());
        assert_eq!(guard.usage_of(MemoryRegion::RuntimeHeap), 1024 * 1024);
    }

    #[test]
    fn test_grow_denied_does_not_update_usage() {
        let mut guard = RegionGuard::new();
        assert!(guard.try_grow_heap(100 * 1024 * 1024).is_ok());
        let before = guard.usage_of(MemoryRegion::RuntimeHeap);
        // 再嘗試 33MB → RuntimeHeap 100+33=133 > 132 → RegionOverflow，usage 不應改變
        let result = guard.try_grow_heap(33 * 1024 * 1024);
        assert!(matches!(result, Err(MemoryError::RegionOverflow { .. })));
        assert_eq!(guard.usage_of(MemoryRegion::RuntimeHeap), before);
    }

    #[test]
    fn test_grow_denied_total_cap_does_not_update_usage() {
        let mut guard = RegionGuard::new();
        guard.record_allocation(MemoryRegion::EcsHeap, 400 * 1024 * 1024);
        let before = guard.usage_of(MemoryRegion::RuntimeHeap);
        // 400+120=520 > 512 → TotalCapExceeded
        let result = guard.try_grow_heap(120 * 1024 * 1024);
        assert!(matches!(result, Err(MemoryError::TotalCapExceeded { .. })));
        assert_eq!(guard.usage_of(MemoryRegion::RuntimeHeap), before);
    }

    #[test]
    fn test_grow_region_overflow() {
        let mut guard = RegionGuard::new();
        // 133MB > 132MB RuntimeHeap 上限
        let result = guard.try_grow_heap(133 * 1024 * 1024);
        assert!(matches!(result, Err(MemoryError::RegionOverflow { .. })));
    }

    #[test]
    fn test_grow_updates_total_usage() {
        let mut guard = RegionGuard::new();
        guard.record_allocation(MemoryRegion::EcsHeap, 100 * 1024 * 1024);
        assert!(guard.try_grow_heap(50 * 1024 * 1024).is_ok());
        assert_eq!(guard.total_usage(), 150 * 1024 * 1024);
    }
}
