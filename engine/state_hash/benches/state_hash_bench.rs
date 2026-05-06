//! State hash 效能基準測試（criterion）。
//!
//! 測量 100/1,000/10,000 entities 的 state hash 計算時間。
//! 效能目標：
//! - 100 entities: < 0.05ms
//! - 1,000 entities: < 0.2ms
//! - 10,000 entities: < 1.5ms

use bridge_types::ecs_mirror::MirroredEntity;
use bridge_types::handles::{EntityId, EntityState};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use deterministic::{SoftF32, SoftVec3};
use state_hash::compute_state_hash;
use std::collections::BTreeMap;

/// 生成 N 個測試用 MirroredEntity
///
/// 使用固定資料，不使用 rand::random() 或 OsRng（符合 Determinism Rules）。
fn generate_test_entities(count: usize) -> BTreeMap<EntityId, MirroredEntity> {
    let mut entities = BTreeMap::new();
    for i in 0..count {
        let id = EntityId(i as u64);
        let x = SoftF32::from_f32(i as f32);
        let y = SoftF32::from_f32(i as f32 * 2.0);
        let z = SoftF32::ZERO;
        let entity = MirroredEntity {
            position: SoftVec3::new(x, y, z),
            rotation: SoftVec3::new(SoftF32::ZERO, SoftF32::ZERO, SoftF32::ZERO),
            scale: SoftVec3::new(
                SoftF32::from_f32(1.0),
                SoftF32::from_f32(1.0),
                SoftF32::from_f32(1.0),
            ),
            hp: 100,
            max_hp: 100,
            state: EntityState(0),
            animation_id: Some(0),
            custom: BTreeMap::new(),
        };
        entities.insert(id, entity);
    }
    entities
}

fn bench_state_hash(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_hash");

    for entity_count in [100usize, 1_000, 10_000] {
        let entities = generate_test_entities(entity_count);
        let rng_state: [u8; 16] = [0u8; 16];
        let tick = 1000u64;

        group.throughput(Throughput::Elements(entity_count as u64));
        group.bench_with_input(
            BenchmarkId::new("compute", entity_count),
            &entity_count,
            |b, _| {
                b.iter(|| {
                    compute_state_hash(
                        criterion::black_box(&entities),
                        criterion::black_box(&rng_state),
                        criterion::black_box(tick),
                    )
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_state_hash);
criterion_main!(benches);

// ── 確定性驗證測試 ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {

    #[test]
    fn test_generate_test_entities_deterministic() {
        let a = generate_test_entities(100);
        let b = generate_test_entities(100);
        assert_eq!(a, b, "兩次生成應產出相同 entities");
    }

    #[test]
    fn test_generate_test_entities_animation_none() {
        let mut entities_some = generate_test_entities(1);
        let mut entities_none = generate_test_entities(1);

        // 修改其中一個的 animation_id 為 None
        entities_none.get_mut(&EntityId(0)).unwrap().animation_id = None;

        let rng_state: [u8; 16] = [0u8; 16];
        let hash_some = compute_state_hash(&entities_some, &rng_state, 1);
        let hash_none = compute_state_hash(&entities_none, &rng_state, 1);

        assert_ne!(
            hash_some, hash_none,
            "animation_id None 與 Some(0) 應產生不同 hash"
        );
    }
}
