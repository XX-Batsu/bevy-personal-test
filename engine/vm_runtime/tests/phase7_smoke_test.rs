//! Phase 7 產出煙霧測試（Phase 8 Task 00）
//!
//! 驗證 Phase 6/7 產出的所有公開型別與方法可從外部 crate 使用且簽名正確。
//! 不測試邊界行為（已在各模組內部測試覆蓋），僅確認 public API 可用性。

use bridge_types::{DeterministicValue, EffectHandle, SoundHandle};
use deterministic::SoftF32;
use vm_runtime::{
    emit_disable_notification, to_deterministic, to_rhai_dynamic, DisableReason, FrameBudget,
    FrameOpsEntry, FrameOpsMetric, HandleRegistry, OpsCostTable, OpsTracker, SandboxedEngine,
    ScopeLimitError, ScopeLimiter, ScriptDisabled,
};

// ═══════════════════════════════════════════════════════════════
// ScopeLimiter 煙霧測試
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_scope_limiter_instantiate() {
    let _limiter = ScopeLimiter::new();
}

#[test]
fn test_scope_limiter_check_empty_scope() {
    let scope = rhai::Scope::new();
    assert!(ScopeLimiter::check_scope_sizes(&scope).is_ok());
}

#[test]
fn test_scope_limiter_check_returns_scope_limit_error() {
    let mut scope = rhai::Scope::new();
    // 超過 64 KB 的字串
    scope.push("big_var", "A".repeat(65_537));
    let result = ScopeLimiter::check_scope_sizes(&scope);
    assert!(result.is_err());
    match result.unwrap_err() {
        ScopeLimitError::VariableTooLarge {
            var_name,
            current_bytes,
            limit,
        } => {
            assert_eq!(var_name, "big_var");
            assert_eq!(current_bytes, 65_537);
            assert_eq!(limit, 65_536);
        }
        other => panic!("預期 VariableTooLarge，實際: {:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════
// OpsTracker 煙霧測試
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_ops_tracker_instantiate() {
    let tracker = OpsTracker::new();
    assert_eq!(tracker.remaining(), 50_000);
}

#[test]
fn test_ops_tracker_deduct() {
    let mut tracker = OpsTracker::new();
    assert!(tracker.deduct(10).is_ok());
    assert_eq!(tracker.remaining(), 49_990);
}

#[test]
fn test_ops_tracker_deduct_exact_limit() {
    let mut tracker = OpsTracker::new();
    assert!(tracker.deduct(50_000).is_ok());
    assert_eq!(tracker.remaining(), 0);
}

#[test]
fn test_ops_tracker_deduct_overflow() {
    let mut tracker = OpsTracker::new();
    let result = tracker.deduct(50_001);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("ops budget exceeded"));
}

#[test]
fn test_ops_tracker_deduct_error_format() {
    let mut tracker = OpsTracker::with_limit(10);
    let err = tracker.deduct(11).unwrap_err();
    assert!(
        err.contains("remaining=10"),
        "錯誤訊息應含 remaining: {}",
        err
    );
    assert!(err.contains("cost=11"), "錯誤訊息應含 cost: {}", err);
}

#[test]
fn test_ops_tracker_deduct_fail_remaining_unchanged() {
    let mut tracker = OpsTracker::with_limit(10);
    let _ = tracker.deduct(11);
    assert_eq!(tracker.remaining(), 10); // 先扣後做不變式
}

#[test]
fn test_ops_tracker_reset() {
    let mut tracker = OpsTracker::new();
    tracker.deduct(100).unwrap();
    tracker.reset();
    assert_eq!(tracker.remaining(), 50_000);
}

// ═══════════════════════════════════════════════════════════════
// OpsCostTable 煙霧測試
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_ops_cost_table_known_api() {
    assert_eq!(OpsCostTable::cost_for("spawn_entity"), 50);
}

#[test]
fn test_ops_cost_table_unknown_api() {
    assert_eq!(OpsCostTable::cost_for("nonexistent"), 1);
}

// ═══════════════════════════════════════════════════════════════
// HandleRegistry 煙霧測試
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_handle_registry_create_effect() {
    let mut registry = HandleRegistry::new();
    let handle = registry.create_effect(0);
    assert!(registry.is_valid_effect(handle));
}

#[test]
fn test_handle_registry_create_sound() {
    let mut registry = HandleRegistry::new();
    let handle = registry.create_sound(0);
    assert!(registry.is_valid_sound(handle));
}

#[test]
fn test_handle_registry_is_valid() {
    let mut registry = HandleRegistry::new();
    let handle = registry.create_effect(0);
    assert!(registry.is_valid_effect(handle));
}

#[test]
fn test_handle_registry_sweep_expired() {
    let mut registry = HandleRegistry::new();
    let handle = registry.create_effect(0);
    // 300 幀後過期（300 - 0 = 300，300 ≮ 300 → 移除）
    registry.sweep_expired(300);
    assert!(!registry.is_valid_effect(handle));
}

#[test]
fn test_handle_registry_sweep_not_expired() {
    let mut registry = HandleRegistry::new();
    let handle = registry.create_effect(0);
    // 299 幀尚未過期（299 - 0 = 299，299 < 300 → 保留）
    registry.sweep_expired(299);
    assert!(registry.is_valid_effect(handle));
}

#[test]
fn test_handle_registry_effect_sound_isolation() {
    let mut registry = HandleRegistry::new();
    let effect = registry.create_effect(0);
    let sound = registry.create_sound(0);
    // effect id 不在 sound map
    assert!(!registry.is_valid_sound(SoundHandle(effect.0)));
    // sound id 不在 effect map
    assert!(!registry.is_valid_effect(EffectHandle(sound.0)));
}

#[test]
fn test_handle_registry_invalidate_nonexistent() {
    let mut registry = HandleRegistry::new();
    // 不存在的 handle → 靜默忽略，不 panic
    registry.invalidate_effect(EffectHandle(9999));
    registry.invalidate_sound(SoundHandle(9999));
}

// ═══════════════════════════════════════════════════════════════
// DisableReason / ScriptDisabled 煙霧測試
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_script_disabled_instantiate() {
    let disabled = ScriptDisabled::new("test", DisableReason::Timeout, 42);
    assert_eq!(disabled.disabled_at_tick, 42);
    assert_eq!(disabled.reason, DisableReason::Timeout);
    assert_eq!(disabled.script_id, "test");
}

#[test]
fn test_disable_reason_all_4_variants() {
    let variants = [
        DisableReason::Timeout,
        DisableReason::OperationLimit,
        DisableReason::ScopeLimitExceeded,
        DisableReason::InitFailed,
    ];
    // 4 variants 完整性
    assert_eq!(variants.len(), 4);
    // 兩兩不等
    for i in 0..variants.len() {
        for j in 0..variants.len() {
            if i == j {
                assert_eq!(variants[i], variants[j]);
            } else {
                assert_ne!(variants[i], variants[j]);
            }
        }
    }
}

#[test]
fn test_disable_reason_scope_limit() {
    let _reason = DisableReason::ScopeLimitExceeded;
}

#[test]
fn test_disable_reason_init_failed() {
    let _reason = DisableReason::InitFailed;
}

// ═══════════════════════════════════════════════════════════════
// FrameOpsEntry 煙霧測試
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_frame_ops_entry_empty_per_script() {
    let entry = FrameOpsEntry {
        frame: 0,
        per_script: vec![],
        total_rhai_ops: 0,
        total_bridge_ops: 0,
        time_ms: 0.0,
    };
    assert_eq!(entry.total_rhai_ops, 0);
    assert_eq!(entry.total_bridge_ops, 0);
}

#[test]
fn test_frame_ops_entry_sum_consistency() {
    let entry = FrameOpsEntry {
        frame: 1,
        per_script: vec![
            ("a".to_string(), 100, 20, 0.5),
            ("b".to_string(), 200, 30, 0.6),
            ("c".to_string(), 300, 50, 0.4),
        ],
        total_rhai_ops: 600,
        total_bridge_ops: 100,
        time_ms: 1.5,
    };
    let sum_rhai: u64 = entry.per_script.iter().map(|(_, r, _, _)| r).sum();
    let sum_bridge: u64 = entry.per_script.iter().map(|(_, _, b, _)| b).sum();
    assert_eq!(entry.total_rhai_ops, sum_rhai);
    assert_eq!(entry.total_bridge_ops, sum_bridge);
}

// ═══════════════════════════════════════════════════════════════
// dynamic_convert 煙霧測試
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_to_deterministic_int() {
    let dyn_val = rhai::Dynamic::from(42_i64);
    assert_eq!(to_deterministic(&dyn_val), DeterministicValue::Int(42));
}

#[test]
fn test_to_deterministic_float() {
    let dyn_val = rhai::Dynamic::from(2.78_f64);
    let expected = DeterministicValue::Float(SoftF32::from_f64(2.78));
    assert_eq!(to_deterministic(&dyn_val), expected);
}

#[test]
fn test_to_deterministic_bool() {
    let dyn_val = rhai::Dynamic::from(true);
    assert_eq!(to_deterministic(&dyn_val), DeterministicValue::Bool(true));
}

#[test]
fn test_to_deterministic_unit() {
    assert_eq!(
        to_deterministic(&rhai::Dynamic::UNIT),
        DeterministicValue::Unit
    );
}

#[test]
fn test_to_deterministic_str() {
    let dyn_val = rhai::Dynamic::from("hello".to_string());
    assert_eq!(
        to_deterministic(&dyn_val),
        DeterministicValue::Str("hello".to_string())
    );
}

#[test]
fn test_to_rhai_dynamic_int() {
    let det = DeterministicValue::Int(42);
    let dyn_val = to_rhai_dynamic(&det);
    assert_eq!(dyn_val.clone_cast::<i64>(), 42);
}

#[test]
fn test_to_rhai_dynamic_float() {
    let sf = SoftF32::from_f64(3.14);
    let det = DeterministicValue::Float(sf);
    let dyn_val = to_rhai_dynamic(&det);
    let f = dyn_val.clone_cast::<f64>();
    assert!((f - sf.to_f64()).abs() < f64::EPSILON);
}

#[test]
fn test_to_rhai_dynamic_bool() {
    let det = DeterministicValue::Bool(false);
    let dyn_val = to_rhai_dynamic(&det);
    assert!(!dyn_val.clone_cast::<bool>());
}

#[test]
fn test_to_rhai_dynamic_unit() {
    let det = DeterministicValue::Unit;
    let dyn_val = to_rhai_dynamic(&det);
    assert!(dyn_val.is_unit());
}

#[test]
fn test_to_rhai_dynamic_str() {
    let det = DeterministicValue::Str("hello".to_string());
    let dyn_val = to_rhai_dynamic(&det);
    assert_eq!(
        dyn_val.clone_cast::<rhai::ImmutableString>().as_str(),
        "hello"
    );
}

#[test]
fn test_dynamic_convert_round_trip_all_variants() {
    let cases: Vec<rhai::Dynamic> = vec![
        rhai::Dynamic::from(42_i64),
        rhai::Dynamic::from(3.14_f64),
        rhai::Dynamic::from(true),
        rhai::Dynamic::UNIT,
        rhai::Dynamic::from("round-trip".to_string()),
    ];
    for original in &cases {
        let det = to_deterministic(original);
        let back = to_rhai_dynamic(&det);
        let det2 = to_deterministic(&back);
        assert_eq!(det, det2, "round-trip 失敗: {:?}", det);
    }
}

// ═══════════════════════════════════════════════════════════════
// SandboxedEngine 煙霧測試
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_sandboxed_engine_new() {
    let _engine = SandboxedEngine::new();
}

#[test]
fn test_sandboxed_engine_execute() {
    let engine = SandboxedEngine::new();
    let result = engine.execute("40 + 2").unwrap();
    assert_eq!(result.as_int().unwrap(), 42);
}

// ═══════════════════════════════════════════════════════════════
// FrameBudget 煙霧測試
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_frame_budget_new_no_params() {
    let budget = FrameBudget::new();
    assert_eq!(budget.remaining_ms(), 4.0);
}

#[test]
fn test_frame_budget_reset() {
    let mut budget = FrameBudget::new();
    budget.deduct_elapsed(2.0);
    assert!(budget.remaining_ms() < 4.0);
    budget.reset();
    assert_eq!(budget.remaining_ms(), 4.0);
}

// ═══════════════════════════════════════════════════════════════
// emit_disable_notification 煙霧測試
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_emit_disable_notification_returns_toast() {
    let events = emit_disable_notification("test.rhai", &DisableReason::Timeout, &[]);
    assert_eq!(events.len(), 1);
}

// ═══════════════════════════════════════════════════════════════
// FrameOpsMetric 煙霧測試
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_frame_ops_metric_record_and_query() {
    let mut metric = FrameOpsMetric::new();
    metric.record(1, "test".to_string(), 100, 50, 0.5);
    assert_eq!(metric.total_rhai_ops(), 100);
    assert_eq!(metric.history(10).len(), 1);
}
