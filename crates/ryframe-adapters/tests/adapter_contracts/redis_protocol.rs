use ryframe_adapters::{
    RedisNamespace,
    idempotency::{
        bounded_redis_ttl_secs, idempotency_guard_key, idempotency_meta_key,
        idempotency_response_key,
    },
    monitor::{CacheCommandStatsStatus, parse_redis_command_stats, parse_redis_info},
};
use ryframe_config::ResourceScopeId;

#[test]
fn namespace_is_idempotent_and_cannot_escape_to_another_scope() {
    let scope = ResourceScopeId::parse("dev-a").expect("作用域有效");
    let namespace = RedisNamespace::for_scope(&scope);
    assert_eq!(namespace.key("jobs:wakeup"), "ryframe:{dev-a}:jobs:wakeup");
    assert_eq!(
        namespace.key("ryframe:v0.5:lock:tenant"),
        "ryframe:{dev-a}:v0.5:lock:tenant"
    );
    assert_eq!(
        namespace.key("ryframe:{dev-a}:jobs:wakeup"),
        "ryframe:{dev-a}:jobs:wakeup"
    );
    assert_eq!(
        namespace.key("ryframe:{other}:jobs:wakeup"),
        "ryframe:{dev-a}:{other}:jobs:wakeup"
    );
}

#[test]
fn idempotency_keys_keep_stable_namespace_and_distinct_suffixes() {
    assert_eq!(
        idempotency_meta_key("request"),
        "ryframe:v0.7:idempotency:request:meta"
    );
    assert_eq!(
        idempotency_response_key("request"),
        "ryframe:v0.7:idempotency:request:response"
    );
    assert_eq!(
        idempotency_guard_key("request"),
        "ryframe:v0.7:idempotency:request:guard"
    );
    assert_eq!(bounded_redis_ttl_secs(u64::MAX), i64::MAX);
}

#[test]
fn redis_info_parser_ignores_headers_and_blank_lines() {
    let parsed =
        parse_redis_info("# Server\r\nredis_version:7.4.0\r\n\r\nredis_mode:standalone\r\n");

    assert_eq!(
        parsed.get("redis_version").map(String::as_str),
        Some("7.4.0")
    );
    assert_eq!(
        parsed.get("redis_mode").map(String::as_str),
        Some("standalone")
    );
    assert_eq!(parsed.len(), 2);
}

#[test]
fn command_stats_parser_keeps_only_command_entries() {
    let parsed = parse_redis_command_stats(
        "# Commandstats\r\ncmdstat_get:calls=3,usec=5\r\nignored:value\r\n",
    );

    assert_eq!(
        parsed.get("get").map(String::as_str),
        Some("calls=3,usec=5")
    );
    assert_eq!(parsed.len(), 1);
}

#[test]
fn unavailable_status_remains_distinct_from_not_configured() {
    assert_ne!(
        CacheCommandStatsStatus::Unavailable,
        CacheCommandStatsStatus::NotConfigured
    );
}
