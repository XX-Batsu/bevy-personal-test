//! Manifest 差異計算 — 泛型 diff 結果，供 AssetManifest 與 ScriptManifest 共用。

/// 泛型 diff 結果。
pub struct ManifestDiff<Id> {
    pub added: Vec<Id>,
    pub updated: Vec<Id>, // blake3 不同
    pub removed: Vec<Id>,
}

/// 支援 diff 計算的 manifest trait，供 AssetManifest 與未來 ScriptManifest 共用。
pub trait ManifestDiffable {
    type Id: Ord + Clone;
    fn diff(&self, newer: &Self) -> ManifestDiff<Self::Id>;
    fn version(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::AssetManifest;
    use crate::AssetId;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    fn manifest_with_assets(entries: &[(&str, &str)]) -> String {
        let mut assets = String::new();
        for (id, hash) in entries {
            assets.push_str(&format!(
                r#"        "{id}": AssetEntry(
            path: "{id}.png",
            format: Png,
            size_bytes: 1000,
            blake3: "{hash}",
            encrypted: false,
            sidecars: [],
            tags: [],
        ),
"#
            ));
        }
        format!(
            r#"AssetManifest(
    version: "1.0.0",
    generated_at: "2026-04-09T12:00:00Z",
    assets: {{
{assets}    }},
)"#
        )
    }

    fn hash_a() -> &'static str {
        "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
    }

    fn hash_b() -> &'static str {
        "b1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
    }

    #[test]
    fn test_diff_added() {
        let old = AssetManifest::parse(&manifest_with_assets(&[("a", hash_a())])).unwrap();
        let new = AssetManifest::parse(&manifest_with_assets(&[("a", hash_a()), ("b", hash_b())]))
            .unwrap();
        let diff = old.diff(&new);
        assert_eq!(diff.added, vec![AssetId::new("b")]);
        assert!(diff.updated.is_empty());
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn test_diff_updated() {
        let old = AssetManifest::parse(&manifest_with_assets(&[("a", hash_a())])).unwrap();
        let new = AssetManifest::parse(&manifest_with_assets(&[("a", hash_b())])).unwrap();
        let diff = old.diff(&new);
        assert!(diff.added.is_empty());
        assert_eq!(diff.updated, vec![AssetId::new("a")]);
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn test_diff_removed() {
        let old = AssetManifest::parse(&manifest_with_assets(&[("a", hash_a()), ("b", hash_b())]))
            .unwrap();
        let new = AssetManifest::parse(&manifest_with_assets(&[("a", hash_a())])).unwrap();
        let diff = old.diff(&new);
        assert!(diff.added.is_empty());
        assert!(diff.updated.is_empty());
        assert_eq!(diff.removed, vec![AssetId::new("b")]);
    }

    #[test]
    fn test_diff_unchanged() {
        let old = AssetManifest::parse(&manifest_with_assets(&[("a", hash_a())])).unwrap();
        let new = AssetManifest::parse(&manifest_with_assets(&[("a", hash_a())])).unwrap();
        let diff = old.diff(&new);
        assert!(diff.added.is_empty());
        assert!(diff.updated.is_empty());
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn test_diff_mixed() {
        let old = AssetManifest::parse(&manifest_with_assets(&[
            ("keep", hash_a()),
            ("update_me", hash_a()),
            ("remove_me", hash_a()),
        ]))
        .unwrap();
        let new = AssetManifest::parse(&manifest_with_assets(&[
            ("add_me", hash_b()),
            ("keep", hash_a()),
            ("update_me", hash_b()),
        ]))
        .unwrap();
        let diff = old.diff(&new);
        assert_eq!(diff.added, vec![AssetId::new("add_me")]);
        assert_eq!(diff.updated, vec![AssetId::new("update_me")]);
        assert_eq!(diff.removed, vec![AssetId::new("remove_me")]);
    }

    /// 產生隨機 manifest RON 字串。
    fn arb_manifest(max_assets: usize) -> impl Strategy<Value = AssetManifest> {
        prop::collection::btree_map("[a-z]{1,5}", "[0-9a-f]{64}", 0..=max_assets).prop_map(
            |entries| {
                let mut assets_str = String::new();
                for (id, hash) in &entries {
                    assets_str.push_str(&format!(
                        r#"        "{id}": AssetEntry(
            path: "{id}.png",
            format: Png,
            size_bytes: 1000,
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
            },
        )
    }

    proptest! {
        #[test]
        fn prop_diff_added_updated_removed_are_disjoint(
            old in arb_manifest(5),
            new in arb_manifest(5),
        ) {
            let diff = old.diff(&new);

            let added: BTreeSet<_> = diff.added.iter().collect();
            let updated: BTreeSet<_> = diff.updated.iter().collect();
            let removed: BTreeSet<_> = diff.removed.iter().collect();

            prop_assert!(added.is_disjoint(&updated));
            prop_assert!(added.is_disjoint(&removed));
            prop_assert!(updated.is_disjoint(&removed));
        }

        #[test]
        fn prop_diff_covers_all_keys(
            old in arb_manifest(5),
            new in arb_manifest(5),
        ) {
            let diff = old.diff(&new);

            for id in new.all_ids() {
                let in_added = diff.added.contains(id);
                let in_updated = diff.updated.contains(id);
                let in_old = old.get(id).is_some();

                if !in_old {
                    prop_assert!(in_added, "新增的 {id} 應在 added 中");
                } else {
                    let old_entry = old.get(id).unwrap();
                    let new_entry = new.get(id).unwrap();
                    if old_entry.blake3_bytes != new_entry.blake3_bytes {
                        prop_assert!(in_updated, "更新的 {id} 應在 updated 中");
                    }
                }
            }

            for id in old.all_ids() {
                if new.get(id).is_none() {
                    prop_assert!(diff.removed.contains(id), "移除的 {id} 應在 removed 中");
                }
            }
        }
    }
}
