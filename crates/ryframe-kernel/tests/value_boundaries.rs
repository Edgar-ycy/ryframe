use std::net::{IpAddr, Ipv4Addr};

use ryframe_kernel::{
    AppError, ExportCursorWindow, IpCidr, MAX_SNOWFLAKE_WORKER_ID, PageResult, PaginationPolicy,
    SnowflakeWorkerId, TenantId, ValidatedPageQuery,
};

const PAGINATION_POLICY: PaginationPolicy = PaginationPolicy::new(10, 100);

#[test]
fn tenant_id_accepts_bounded_ascii_without_allocating() {
    let source = String::from("tenant_01-a");
    let tenant_id = TenantId::parse(&source).unwrap();

    assert_eq!(tenant_id.as_str(), source);
    assert_eq!(tenant_id.as_str().as_ptr(), source.as_ptr());
}

#[test]
fn tenant_id_rejects_unsafe_or_ambiguous_values() {
    for value in ["a", "-tenant", "tenant-", "租户", "tenant:*", "a b"] {
        assert!(TenantId::parse(value).is_err(), "{value} 应被拒绝");
    }
}

#[test]
fn snowflake_worker_id_is_bounded_by_encoded_bits() {
    assert_eq!(
        SnowflakeWorkerId::new(0).map(SnowflakeWorkerId::get),
        Some(0)
    );
    assert!(SnowflakeWorkerId::new(MAX_SNOWFLAKE_WORKER_ID).is_some());
    assert!(SnowflakeWorkerId::new(-1).is_none());
    assert!(SnowflakeWorkerId::new(MAX_SNOWFLAKE_WORKER_ID + 1).is_none());
}

#[test]
fn cidr_normalizes_network_and_checks_membership() {
    let cidr = IpCidr::parse("10.0.1.9/24").expect("解析 CIDR");
    assert!(cidr.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 200))));
    assert!(!cidr.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 2, 1))));
}

#[test]
fn pagination_applies_defaults_and_calculates_offset() {
    let query = ValidatedPageQuery::from_optional(Some(3), None, PAGINATION_POLICY).unwrap();

    assert_eq!(query.page(), 3);
    assert_eq!(query.page_size(), 10);
    assert_eq!(query.offset(), 20);
}

#[test]
fn pagination_rejects_invalid_requests_and_overflow() {
    assert!(matches!(
        ValidatedPageQuery::new(0, 10, PAGINATION_POLICY),
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        ValidatedPageQuery::new(1, 101, PAGINATION_POLICY),
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        ValidatedPageQuery::new(u64::MAX, 2, PAGINATION_POLICY),
        Err(AppError::Validation(_))
    ));
}

#[test]
fn page_result_calculates_total_pages_without_copying_records() {
    let query = ValidatedPageQuery::new(1, 10, PAGINATION_POLICY).unwrap();
    let result = PageResult::new(vec!["a", "b"], 21, &query);

    assert_eq!(result.total_pages(), 3);
    assert_eq!(result.records, vec!["a", "b"]);
}

#[test]
fn export_cursor_window_preserves_bounds_and_is_copy() {
    let window = ExportCursorWindow::new(Some(41), 99, 1_000);
    let copied = window;

    assert_eq!(window.after_id(), Some(41));
    assert_eq!(copied.upper_id(), 99);
    assert_eq!(copied.limit(), 1_000);
}
