//! OtaManager — 統一 asset + script OTA 差異管理。

use asset_manifest::{AssetManifest, ManifestDiffable};
use std::collections::BTreeMap;

use crate::priority::{OtaItem, Priority};

/// WASM 記憶體預算：同時 OTA 素材總大小上限 64 MB。
const OTA_MEMORY_BUDGET: u64 = 64 * 1024 * 1024;

/// OTA 更新管理器。
///
/// 目前僅支援 Asset OTA。Script OTA 將在 ScriptManifest 實作時
/// 透過 ManifestDiffable trait 加入。屆時此 struct 會改為
/// `OtaManager<S: ManifestDiffable>` 泛型版本。
pub struct OtaManager {
    current_manifest: AssetManifest,
    download_queue: BTreeMap<Priority, Vec<OtaItem>>,
    in_progress_bytes: u64,
}

impl OtaManager {
    pub fn new(manifest: AssetManifest) -> Self {
        Self {
            current_manifest: manifest,
            download_queue: BTreeMap::new(),
            in_progress_bytes: 0,
        }
    }

    /// 接收新 manifest，計算 diff 並排程下載。
    pub fn schedule_update(&mut self, newer: &AssetManifest) -> Vec<OtaItem> {
        let diff = self.current_manifest.diff(newer);
        let mut items = Vec::new();

        for id in diff.added.iter().chain(diff.updated.iter()) {
            if let Some(entry) = newer.get(id) {
                let item = OtaItem::Asset {
                    id: id.clone(),
                    blake3: entry.blake3_bytes,
                    size_bytes: entry.size_bytes,
                };
                let priority = item.priority();
                self.download_queue
                    .entry(priority)
                    .or_default()
                    .push(item.clone());
                items.push(item);
            }
        }

        items
    }

    /// 取出下一批可下載項目（受記憶體預算限制）。
    pub fn next_batch(&mut self) -> Vec<OtaItem> {
        let mut batch = Vec::new();

        for items in self.download_queue.values_mut() {
            let mut i = 0;
            while i < items.len() {
                let size = match &items[i] {
                    OtaItem::Asset { size_bytes, .. } => *size_bytes,
                    OtaItem::Script { .. } => 0,
                };
                if self.in_progress_bytes + size <= OTA_MEMORY_BUDGET {
                    self.in_progress_bytes += size;
                    batch.push(items.remove(i));
                } else {
                    i += 1;
                }
            }
        }

        self.download_queue.retain(|_, v| !v.is_empty());
        batch
    }

    /// 標記項目下載完成。
    pub fn complete_item(&mut self, item: &OtaItem) {
        let size = match item {
            OtaItem::Asset { size_bytes, .. } => *size_bytes,
            OtaItem::Script { .. } => 0,
        };
        self.in_progress_bytes = self.in_progress_bytes.saturating_sub(size);
    }

    /// 套用新 manifest。
    pub fn apply_manifest(&mut self, manifest: AssetManifest) {
        self.current_manifest = manifest;
        tracing::info!(
            "OTA manifest 已更新至版本 {}",
            self.current_manifest.version
        );
    }

    pub fn in_progress_bytes(&self) -> u64 {
        self.in_progress_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with_assets(entries: &[(&str, &str, u64)]) -> AssetManifest {
        let mut assets_str = String::new();
        for (id, hash, size) in entries {
            assets_str.push_str(&format!(
                r#"        "{id}": AssetEntry(
            path: "{id}.png",
            format: Png,
            size_bytes: {size},
            blake3: "{hash}",
            encrypted: false,
            sidecars: [],
            tags: [],
        ),
"#
            ));
        }
        let ron = format!(
            r#"AssetManifest(
    version: "1.0.0",
    generated_at: "2026-04-09T12:00:00Z",
    assets: {{
{assets_str}    }},
)"#
        );
        AssetManifest::parse(&ron).unwrap()
    }

    fn hash_a() -> &'static str {
        "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
    }

    fn hash_b() -> &'static str {
        "b1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
    }

    #[test]
    fn test_schedule_update_detects_changes() {
        let old = manifest_with_assets(&[("a", hash_a(), 1000)]);
        let new = manifest_with_assets(&[("a", hash_b(), 1000), ("b", hash_b(), 2000)]);
        let mut mgr = OtaManager::new(old);
        let items = mgr.schedule_update(&new);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn test_next_batch_respects_memory_budget() {
        let old = manifest_with_assets(&[]);
        let new = manifest_with_assets(&[
            ("big", hash_a(), 50_000_000),
            ("medium", hash_b(), 20_000_000),
        ]);
        let mut mgr = OtaManager::new(old);
        mgr.schedule_update(&new);

        let batch1 = mgr.next_batch();
        assert_eq!(batch1.len(), 1);
        assert_eq!(mgr.in_progress_bytes(), 50_000_000);

        let batch2 = mgr.next_batch();
        assert!(batch2.is_empty());

        mgr.complete_item(&batch1[0]);
        let batch3 = mgr.next_batch();
        assert_eq!(batch3.len(), 1);
    }

    #[test]
    fn test_priority_ordering() {
        let old = manifest_with_assets(&[]);
        let new =
            manifest_with_assets(&[("small", hash_a(), 500_000), ("big", hash_b(), 2_000_000)]);
        let mut mgr = OtaManager::new(old);
        mgr.schedule_update(&new);

        let batch = mgr.next_batch();
        assert_eq!(batch.len(), 2);
        match &batch[0] {
            OtaItem::Asset { id, .. } => assert_eq!(id.as_str(), "small"),
            OtaItem::Script { .. } => panic!("預期 Asset 項目"),
        }
    }
}
