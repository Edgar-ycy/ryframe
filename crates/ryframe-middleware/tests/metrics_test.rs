use std::time::Duration;

use ryframe_middleware::metrics::{
    database_read_fallback_total, database_read_selection_totals, metrics_text, normalize_path,
    observe_job_duration, observe_message_ack_latency, record_database_read_fallback,
    record_database_read_selection, record_otel_exporter_failure, set_database_node_health,
    set_job_oldest_ready_age, set_job_queue_depth, set_otel_exporter_degraded,
};

#[test]
fn test_normalize_path_static() {
    assert_eq!(normalize_path("/api/v1/login"), "/api/v1/login");
    assert_eq!(normalize_path("/metrics"), "/metrics");
    assert_eq!(normalize_path("/livez"), "/livez");
}

#[test]
fn test_normalize_path_dynamic_id() {
    assert_eq!(normalize_path("/system/user/123"), "/system/user/:id");
    assert_eq!(normalize_path("/system/menu/456"), "/system/menu/:id");
}

#[test]
fn test_normalize_path_uuid() {
    assert_eq!(
        normalize_path("/api/v1/token/550e8400-e29b-41d4-a716-446655440000"),
        "/api/v1/token/:uuid"
    );
}

#[test]
fn test_normalize_path_root() {
    assert_eq!(normalize_path("/"), "/");
    assert_eq!(normalize_path(""), "/");
}

#[test]
fn test_normalize_path_mixed() {
    assert_eq!(
        normalize_path("/system/role/1/user/100"),
        "/system/role/:id/user/:id"
    );
}

#[test]
fn test_metrics_text_format() {
    let text = metrics_text();
    // 未带标签的进程指标会立即导出；带标签的 HTTP
    // 序列只会在该标签集合的首次请求后出现。
    assert!(text.contains("# HELP ryframe_process_cpu_seconds_total"));
    assert!(text.contains("# TYPE ryframe_process_resident_memory_bytes gauge"));
}

#[test]
fn operational_metrics_use_bounded_labels() {
    let fallback_before = database_read_fallback_total();
    set_database_node_health("replica-a", "replica", false);
    record_database_read_selection("primary", "fallback");
    record_database_read_fallback();
    set_job_queue_depth("system.message.dispatch", "pending", 3);
    set_job_oldest_ready_age("system.message.dispatch", Duration::from_secs(90));
    observe_job_duration(
        "system.message.dispatch",
        "succeeded",
        Duration::from_millis(25),
    );
    observe_message_ack_latency(Duration::from_millis(10));
    record_otel_exporter_failure();
    set_otel_exporter_degraded(false);

    let text = metrics_text();
    assert!(text.contains("ryframe_db_node_up"));
    assert!(text.contains("ryframe_db_read_selection_total"));
    assert!(text.contains("ryframe_db_read_fallback_total"));
    assert!(text.contains("ryframe_job_queue_depth"));
    assert!(text.contains("ryframe_job_oldest_ready_age_seconds"));
    assert!(text.contains("ryframe_job_duration_seconds"));
    assert!(text.contains("ryframe_message_ack_latency_seconds"));
    assert!(text.contains("ryframe_otel_exporter_failures_total"));
    assert!(text.contains("ryframe_otel_exporter_degraded"));
    assert_eq!(database_read_fallback_total(), fallback_before + 1);
    assert!(
        database_read_selection_totals()
            .iter()
            .any(|(target, reason, count)| *target == "primary"
                && *reason == "fallback"
                && *count >= 1)
    );
}
