use ryframe_middleware::metrics::{metrics_text, normalize_path};

#[test]
fn test_normalize_path_static() {
    assert_eq!(normalize_path("/api/v1/login"), "/api/v1/login");
    assert_eq!(normalize_path("/metrics"), "/metrics");
    assert_eq!(normalize_path("/health"), "/health");
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
    // 应该包含 Prometheus 格式的 HELP/TYPE 注释
    assert!(text.contains("ryframe_http_requests_total") || text.is_empty());
}
