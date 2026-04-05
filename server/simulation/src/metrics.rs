//! Server 端 Prometheus metrics 集合。
//!
//! 提供 [`ServerMetrics`]：6 個 metric 指標，
//! 以及 `metrics_handler()` 產出 Prometheus text exposition format。

use prometheus::{Gauge, Histogram, HistogramOpts, IntCounter, IntGauge, Opts, Registry};

/// Server 端 Prometheus metrics 集合
pub struct ServerMetrics {
    /// Prometheus registry（用於 gather）
    registry: Registry,
    /// 每房間玩家數（gauge）
    pub room_player_count: IntGauge,
    /// 每 tick 處理時間直方圖（毫秒）
    pub tick_processing_time_ms: Histogram,
    /// State hash 驗證通過率（gauge, 0.0-1.0）
    pub state_hash_validation_rate: Gauge,
    /// OTA 更新成功率（gauge, 0.0-1.0）
    pub ota_update_success_rate: Gauge,
    /// Session 持續時間直方圖（秒）
    pub session_duration_seconds: Histogram,
    /// 重連次數（counter，累計）
    pub reconnection_count: IntCounter,
}

impl ServerMetrics {
    /// 初始化並向 Prometheus registry 註冊所有指標
    ///
    /// # Errors
    /// `prometheus::Error` — 若 metric 名稱衝突或格式不合法
    pub fn new(registry: &Registry) -> Result<Self, prometheus::Error> {
        let room_player_count =
            IntGauge::with_opts(Opts::new("room_player_count", "每房間玩家數"))?;
        registry.register(Box::new(room_player_count.clone()))?;

        let tick_processing_time_ms = Histogram::with_opts(HistogramOpts::new(
            "tick_processing_time_ms",
            "每 tick 處理時間（毫秒）",
        ))?;
        registry.register(Box::new(tick_processing_time_ms.clone()))?;

        let state_hash_validation_rate = Gauge::with_opts(Opts::new(
            "state_hash_validation_rate",
            "State hash 驗證通過率（0.0-1.0）",
        ))?;
        registry.register(Box::new(state_hash_validation_rate.clone()))?;

        let ota_update_success_rate = Gauge::with_opts(Opts::new(
            "ota_update_success_rate",
            "OTA 更新成功率（0.0-1.0）",
        ))?;
        registry.register(Box::new(ota_update_success_rate.clone()))?;

        let session_duration_seconds = Histogram::with_opts(HistogramOpts::new(
            "session_duration_seconds",
            "Session 持續時間（秒）",
        ))?;
        registry.register(Box::new(session_duration_seconds.clone()))?;

        let reconnection_count =
            IntCounter::with_opts(Opts::new("reconnection_count", "重連次數（累計）"))?;
        registry.register(Box::new(reconnection_count.clone()))?;

        Ok(Self {
            registry: registry.clone(),
            room_player_count,
            tick_processing_time_ms,
            state_hash_validation_rate,
            ota_update_success_rate,
            session_duration_seconds,
            reconnection_count,
        })
    }

    /// 產出 Prometheus text exposition format
    ///
    /// 輸出包含 `# HELP` 和 `# TYPE` 行（每個 metric 至少各一行）
    pub fn metrics_handler(&self) -> String {
        use prometheus::Encoder;
        let encoder = prometheus::TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }
}

// ── 測試 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use prometheus::Registry;

    // --- 正常路徑 ---

    #[test]
    fn test_server_metrics_creation() {
        let registry = Registry::new();
        let _metrics = ServerMetrics::new(&registry).unwrap();
        let families = registry.gather();
        assert!(
            families.len() >= 6,
            "應註冊至少 6 個 metric families，實際 {}",
            families.len()
        );
    }

    #[test]
    fn test_room_player_count_updates() {
        let registry = Registry::new();
        let metrics = ServerMetrics::new(&registry).unwrap();
        metrics.room_player_count.set(4);
        let output = metrics.metrics_handler();
        assert!(output.contains("room_player_count 4"));
    }

    #[test]
    fn test_tick_processing_time_records() {
        let registry = Registry::new();
        let metrics = ServerMetrics::new(&registry).unwrap();
        metrics.tick_processing_time_ms.observe(1.5);
        let output = metrics.metrics_handler();
        assert!(output.contains("tick_processing_time_ms_count 1"));
    }

    #[test]
    fn test_state_hash_validation_rate() {
        let registry = Registry::new();
        let metrics = ServerMetrics::new(&registry).unwrap();
        metrics.state_hash_validation_rate.set(0.95);
        let output = metrics.metrics_handler();
        assert!(output.contains("state_hash_validation_rate 0.95"));
    }

    #[test]
    fn test_ota_update_success_rate() {
        let registry = Registry::new();
        let metrics = ServerMetrics::new(&registry).unwrap();
        metrics.ota_update_success_rate.set(1.0);
        let output = metrics.metrics_handler();
        assert!(output.contains("ota_update_success_rate 1"));
    }

    #[test]
    fn test_session_duration_records() {
        let registry = Registry::new();
        let metrics = ServerMetrics::new(&registry).unwrap();
        metrics.session_duration_seconds.observe(120.0);
        let output = metrics.metrics_handler();
        assert!(output.contains("session_duration_seconds_count 1"));
    }

    #[test]
    fn test_reconnection_count_increments() {
        let registry = Registry::new();
        let metrics = ServerMetrics::new(&registry).unwrap();
        metrics.reconnection_count.inc();
        metrics.reconnection_count.inc();
        metrics.reconnection_count.inc();
        let output = metrics.metrics_handler();
        assert!(output.contains("reconnection_count 3"));
    }

    // --- Exposition format 驗證 ---

    #[test]
    fn test_metrics_handler_format() {
        let registry = Registry::new();
        let _metrics = ServerMetrics::new(&registry).unwrap();
        let output = _metrics.metrics_handler();
        assert!(output.contains("# HELP"));
        assert!(output.contains("# TYPE"));
    }

    #[test]
    fn test_metrics_handler_contains_help_and_type_for_all() {
        let registry = Registry::new();
        let metrics = ServerMetrics::new(&registry).unwrap();
        let output = metrics.metrics_handler();
        let metric_names = [
            "room_player_count",
            "tick_processing_time_ms",
            "state_hash_validation_rate",
            "ota_update_success_rate",
            "session_duration_seconds",
            "reconnection_count",
        ];
        for name in &metric_names {
            assert!(
                output.contains(&format!("# HELP {}", name)),
                "缺少 # HELP {}",
                name,
            );
            assert!(
                output.contains(&format!("# TYPE {}", name)),
                "缺少 # TYPE {}",
                name,
            );
        }
    }

    #[test]
    fn test_metrics_handler_multiline_format() {
        let registry = Registry::new();
        let metrics = ServerMetrics::new(&registry).unwrap();
        metrics.room_player_count.set(2);
        metrics.reconnection_count.inc();
        let output = metrics.metrics_handler();
        let line_count = output.lines().count();
        assert!(
            line_count > 6,
            "exposition 應為多行文字，實際 {} 行",
            line_count
        );
    }

    // --- 錯誤路徑 ---

    #[test]
    fn test_duplicate_registration_returns_error() {
        let registry = Registry::new();
        let _metrics1 = ServerMetrics::new(&registry).unwrap();
        let result = ServerMetrics::new(&registry);
        assert!(result.is_err(), "重複註冊應回傳 prometheus::Error");
    }

    // --- 邊界值 ---

    #[test]
    fn test_gauge_boundary_zero() {
        let registry = Registry::new();
        let metrics = ServerMetrics::new(&registry).unwrap();
        metrics.state_hash_validation_rate.set(0.0);
        let output = metrics.metrics_handler();
        assert!(output.contains("state_hash_validation_rate 0"));
    }

    #[test]
    fn test_gauge_boundary_one() {
        let registry = Registry::new();
        let metrics = ServerMetrics::new(&registry).unwrap();
        metrics.ota_update_success_rate.set(1.0);
        let output = metrics.metrics_handler();
        assert!(output.contains("ota_update_success_rate 1"));
    }

    #[test]
    fn test_int_gauge_zero() {
        let registry = Registry::new();
        let _metrics = ServerMetrics::new(&registry).unwrap();
        let output = _metrics.metrics_handler();
        assert!(output.contains("room_player_count 0"));
    }

    #[test]
    fn test_histogram_multiple_observations() {
        let registry = Registry::new();
        let metrics = ServerMetrics::new(&registry).unwrap();
        metrics.tick_processing_time_ms.observe(1.0);
        metrics.tick_processing_time_ms.observe(5.0);
        metrics.tick_processing_time_ms.observe(16.0);
        let output = metrics.metrics_handler();
        assert!(output.contains("tick_processing_time_ms_count 3"));
        assert!(output.contains("tick_processing_time_ms_sum 22"));
    }

    #[test]
    fn test_counter_starts_at_zero() {
        let registry = Registry::new();
        let _metrics = ServerMetrics::new(&registry).unwrap();
        let output = _metrics.metrics_handler();
        assert!(output.contains("reconnection_count 0"));
    }
}
