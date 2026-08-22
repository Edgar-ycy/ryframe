//! Prometheus 指标注册、记录与进程采样。

use std::{sync::LazyLock, time::Duration};

use lazy_static::lazy_static;
use prometheus::{
    Encoder, Gauge, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge,
    IntGaugeVec, Opts, Registry, TextEncoder,
};

static METRICS_REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
    Registry::new_custom(Some("ryframe".to_string()), None)
        .expect("create the RyFrame metrics registry")
});

lazy_static! {
    static ref HTTP_REQUESTS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "http_requests_total",
            "HTTP requests by method, path, and status"
        ),
        &["method", "path", "status"],
    )
    .expect("create http_requests_total");
    static ref HTTP_REQUEST_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("http_request_duration_seconds", "HTTP request latency").buckets(vec![
            0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0
        ]),
        &["method", "path"],
    )
    .expect("create http_request_duration_seconds");
    static ref HTTP_REQUESTS_IN_FLIGHT: IntGauge = IntGauge::new(
        "http_requests_in_flight",
        "HTTP requests currently being handled",
    )
    .expect("create http_requests_in_flight");
    static ref PROCESS_CPU_SECONDS: Gauge = Gauge::new(
        "process_cpu_seconds_total",
        "Total process CPU time in seconds",
    )
    .expect("create process_cpu_seconds_total");
    static ref PROCESS_RESIDENT_MEMORY_BYTES: Gauge = Gauge::new(
        "process_resident_memory_bytes",
        "Resident process memory in bytes",
    )
    .expect("create process_resident_memory_bytes");
    static ref PROCESS_VIRTUAL_MEMORY_BYTES: Gauge = Gauge::new(
        "process_virtual_memory_bytes",
        "Virtual process memory in bytes",
    )
    .expect("create process_virtual_memory_bytes");
    static ref PROCESS_OPEN_FDS: Gauge =
        Gauge::new("process_open_fds", "Open process file descriptors")
            .expect("create process_open_fds");
    static ref PROCESS_THREADS: Gauge =
        Gauge::new("process_threads", "Process thread count").expect("create process_threads");
    static ref PROCESS_START_TIME_SECONDS: Gauge = Gauge::new(
        "process_start_time_seconds",
        "Process start time as a Unix timestamp",
    )
    .expect("create process_start_time_seconds");
    static ref AUTH_REFRESH_REPLAY_TOTAL: IntCounter = IntCounter::new(
        "auth_refresh_replay_total",
        "Confirmed refresh-token replay attempts",
    )
    .expect("create auth_refresh_replay_total");
    static ref AUTH_CSRF_REJECTED_TOTAL: IntCounter = IntCounter::new(
        "auth_csrf_rejected_total",
        "Authentication requests rejected by CSRF validation",
    )
    .expect("create auth_csrf_rejected_total");
    static ref REDIS_DEGRADED_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "redis_degraded_total",
            "Redis degradation events by subsystem"
        ),
        &["subsystem"],
    )
    .expect("create redis_degraded_total");
    static ref REDIS_DEGRADED_STATE: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "redis_degraded_state",
            "Current Redis degradation state by subsystem"
        ),
        &["subsystem"],
    )
    .expect("create redis_degraded_state");
    static ref IDEMPOTENCY_CONFLICTS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "idempotency_conflicts_total",
            "Idempotency conflicts by reason"
        ),
        &["reason"],
    )
    .expect("create idempotency_conflicts_total");
    static ref RATE_LIMIT_REJECTIONS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "rate_limit_rejections_total",
            "Rate-limit rejections by scope"
        ),
        &["scope"],
    )
    .expect("create rate_limit_rejections_total");
    static ref READINESS_FAILURES_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "readiness_failures_total",
            "Readiness failures by dependency"
        ),
        &["dependency"],
    )
    .expect("create readiness_failures_total");
    static ref AUDIT_FAILURES_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "audit_failures_total",
            "Operation-audit failures by bounded processing stage"
        ),
        &["stage"],
    )
    .expect("create audit_failures_total");
    static ref WS_CONNECTIONS: IntGauge = IntGauge::new(
        "ws_connections",
        "Current authenticated message WebSocket connections",
    )
    .expect("create ws_connections");
    static ref WS_TICKETS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new("ws_ticket_total", "WebSocket ticket outcomes by result",),
        &["result"],
    )
    .expect("create ws_ticket_total");
    static ref MESSAGE_DELIVERY_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "message_delivery_total",
            "Message WebSocket delivery outcomes by result",
        ),
        &["result"],
    )
    .expect("create message_delivery_total");
    static ref MESSAGE_REDIS_LISTENER_CONNECTED: IntGauge = IntGauge::new(
        "message_redis_listener_connected",
        "Whether the message Redis listener is currently connected",
    )
    .expect("create message_redis_listener_connected");
    static ref MESSAGE_REPLAY_QUERY_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "message_replay_query_total",
            "Shared inbox replay queries by bounded result",
        ),
        &["result"],
    )
    .expect("create message_replay_query_total");
    static ref MESSAGE_RETENTION_DELETED_TOTAL: IntCounter = IntCounter::new(
        "message_retention_deleted_total",
        "Expired message records removed by retention jobs",
    )
    .expect("create message_retention_deleted_total");
    static ref DB_NODE_UP: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "db_node_up",
            "Database node routing health by configured node name and kind",
        ),
        &["name", "kind"],
    )
    .expect("create db_node_up");
    static ref DB_READ_SELECTION_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "db_read_selection_total",
            "Database read selections by target and bounded reason",
        ),
        &["target", "reason"],
    )
    .expect("create db_read_selection_total");
    static ref DB_READ_FALLBACK_TOTAL: IntCounter = IntCounter::new(
        "db_read_fallback_total",
        "Eventual-consistency reads routed to primary because no healthy replica was available",
    )
    .expect("create db_read_fallback_total");
    static ref TENANT_DATA_TARGETS: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "tenant_data_targets",
            "Tenant-data target count by bounded mode and cached health",
        ),
        &["mode", "health"],
    )
    .expect("create tenant_data_targets");
    static ref TENANT_DATA_PLACEMENTS: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "tenant_data_placements",
            "Tenant-data placement count by bounded state and target mode",
        ),
        &["mode", "state"],
    )
    .expect("create tenant_data_placements");
    static ref TENANT_DATA_POOL_OPEN: IntGauge = IntGauge::new(
        "tenant_data_pool_open",
        "Open tenant-data MySQL pools in this process",
    )
    .expect("create tenant_data_pool_open");
    static ref TENANT_DATA_POOL_OPENING: IntGauge = IntGauge::new(
        "tenant_data_pool_opening",
        "Tenant-data MySQL pools currently opening in this process",
    )
    .expect("create tenant_data_pool_opening");
    static ref TENANT_DATA_POOL_RESERVED_CONNECTIONS: IntGauge = IntGauge::new(
        "tenant_data_pool_reserved_connections",
        "Connections atomically reserved across tenant-data pools",
    )
    .expect("create tenant_data_pool_reserved_connections");
    static ref TENANT_DATA_POOL_ACTIVE_LEASES: IntGauge = IntGauge::new(
        "tenant_data_pool_active_leases",
        "Active tenant-data pool leases in this process",
    )
    .expect("create tenant_data_pool_active_leases");
    static ref JOB_QUEUE_DEPTH: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "job_queue_depth",
            "Persistent background-job counts by registered type and status",
        ),
        &["type", "status"],
    )
    .expect("create job_queue_depth");
    static ref JOB_OLDEST_READY_AGE_SECONDS: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "job_oldest_ready_age_seconds",
            "Age of the oldest ready background job by registered type",
        ),
        &["type"],
    )
    .expect("create job_oldest_ready_age_seconds");
    static ref JOB_DURATION_SECONDS: HistogramVec = HistogramVec::new(
        HistogramOpts::new(
            "job_duration_seconds",
            "Background-job execution duration by registered type and result",
        )
        .buckets(vec![
            0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 15.0, 60.0
        ]),
        &["type", "result"],
    )
    .expect("create job_duration_seconds");
    static ref JOB_CLAIM_ATTEMPTS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "job_claim_attempts_total",
            "Persistent queue claim attempts by queue and bounded result",
        ),
        &["queue", "result"],
    )
    .expect("create job_claim_attempts_total");
    static ref JOB_WAKEUP_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "job_wakeup_total",
            "Local and Redis queue wakeup outcomes by bounded transport and result",
        ),
        &["queue", "transport", "result"],
    )
    .expect("create job_wakeup_total");
    static ref JOB_WAKEUP_LISTENER_UP: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "job_wakeup_listener_up",
            "Whether the current process Redis wakeup listener is connected",
        ),
        &["queue"],
    )
    .expect("create job_wakeup_listener_up");
    static ref JOB_WAKEUP_PROTOCOL_ERRORS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "job_wakeup_protocol_errors_total",
            "Ignored Redis wakeup payloads by bounded validation result",
        ),
        &["result"],
    )
    .expect("create job_wakeup_protocol_errors_total");
    static ref JOB_SCHEDULE_SCAN_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "job_schedule_scan_total",
            "Database-backed schedule scans by bounded result",
        ),
        &["result"],
    )
    .expect("create job_schedule_scan_total");
    static ref JOB_SCHEDULE_TRIGGER_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "job_schedule_trigger_total",
            "Schedule trigger attempts by bounded outcome",
        ),
        &["outcome"],
    )
    .expect("create job_schedule_trigger_total");
    static ref JOB_SCHEDULE_LAG_SECONDS: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "job_schedule_lag_seconds",
            "Delay between a scheduled UTC fire time and database claim time",
        )
        .buckets(vec![
            0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 15.0, 60.0, 300.0
        ]),
    )
    .expect("create job_schedule_lag_seconds");
    static ref AUTHORIZATION_CACHE_LOOKUPS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "authorization_cache_lookups_total",
            "Authorization cache lookup outcomes by bounded scope and result",
        ),
        &["scope", "result"],
    )
    .expect("create authorization_cache_lookups_total");
    static ref MESSAGE_ACK_LATENCY_SECONDS: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "message_ack_latency_seconds",
            "Successful message acknowledgement operation latency",
        )
        .buckets(vec![
            0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0
        ]),
    )
    .expect("create message_ack_latency_seconds");
    static ref OTEL_EXPORTER_FAILURES_TOTAL: IntCounter = IntCounter::new(
        "otel_exporter_failures_total",
        "OpenTelemetry exporter initialization failures",
    )
    .expect("create otel_exporter_failures_total");
    static ref OTEL_EXPORTER_RUNTIME_FAILURES_TOTAL: IntCounter = IntCounter::new(
        "otel_exporter_runtime_failures_total",
        "OpenTelemetry exporter runtime failures",
    )
    .expect("create otel_exporter_runtime_failures_total");
    static ref OTEL_EXPORTER_DEGRADED: IntGauge = IntGauge::new(
        "otel_exporter_degraded",
        "Whether OpenTelemetry exporting is degraded after initialization failure",
    )
    .expect("create otel_exporter_degraded");
    static ref METRICS_REGISTERED: std::sync::Once = std::sync::Once::new();
}

fn ensure_registered() {
    METRICS_REGISTERED.call_once(|| {
        for collector in [
            Box::new(HTTP_REQUESTS_TOTAL.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(HTTP_REQUEST_DURATION.clone()),
            Box::new(HTTP_REQUESTS_IN_FLIGHT.clone()),
            Box::new(PROCESS_CPU_SECONDS.clone()),
            Box::new(PROCESS_RESIDENT_MEMORY_BYTES.clone()),
            Box::new(PROCESS_VIRTUAL_MEMORY_BYTES.clone()),
            Box::new(PROCESS_OPEN_FDS.clone()),
            Box::new(PROCESS_THREADS.clone()),
            Box::new(PROCESS_START_TIME_SECONDS.clone()),
            Box::new(AUTH_REFRESH_REPLAY_TOTAL.clone()),
            Box::new(AUTH_CSRF_REJECTED_TOTAL.clone()),
            Box::new(REDIS_DEGRADED_TOTAL.clone()),
            Box::new(REDIS_DEGRADED_STATE.clone()),
            Box::new(IDEMPOTENCY_CONFLICTS_TOTAL.clone()),
            Box::new(RATE_LIMIT_REJECTIONS_TOTAL.clone()),
            Box::new(READINESS_FAILURES_TOTAL.clone()),
            Box::new(AUDIT_FAILURES_TOTAL.clone()),
            Box::new(WS_CONNECTIONS.clone()),
            Box::new(WS_TICKETS_TOTAL.clone()),
            Box::new(MESSAGE_DELIVERY_TOTAL.clone()),
            Box::new(MESSAGE_REDIS_LISTENER_CONNECTED.clone()),
            Box::new(MESSAGE_REPLAY_QUERY_TOTAL.clone()),
            Box::new(MESSAGE_RETENTION_DELETED_TOTAL.clone()),
            Box::new(DB_NODE_UP.clone()),
            Box::new(DB_READ_SELECTION_TOTAL.clone()),
            Box::new(DB_READ_FALLBACK_TOTAL.clone()),
            Box::new(TENANT_DATA_TARGETS.clone()),
            Box::new(TENANT_DATA_PLACEMENTS.clone()),
            Box::new(TENANT_DATA_POOL_OPEN.clone()),
            Box::new(TENANT_DATA_POOL_OPENING.clone()),
            Box::new(TENANT_DATA_POOL_RESERVED_CONNECTIONS.clone()),
            Box::new(TENANT_DATA_POOL_ACTIVE_LEASES.clone()),
            Box::new(JOB_QUEUE_DEPTH.clone()),
            Box::new(JOB_OLDEST_READY_AGE_SECONDS.clone()),
            Box::new(JOB_DURATION_SECONDS.clone()),
            Box::new(JOB_CLAIM_ATTEMPTS_TOTAL.clone()),
            Box::new(JOB_WAKEUP_TOTAL.clone()),
            Box::new(JOB_WAKEUP_LISTENER_UP.clone()),
            Box::new(JOB_WAKEUP_PROTOCOL_ERRORS_TOTAL.clone()),
            Box::new(JOB_SCHEDULE_SCAN_TOTAL.clone()),
            Box::new(JOB_SCHEDULE_TRIGGER_TOTAL.clone()),
            Box::new(JOB_SCHEDULE_LAG_SECONDS.clone()),
            Box::new(AUTHORIZATION_CACHE_LOOKUPS_TOTAL.clone()),
            Box::new(MESSAGE_ACK_LATENCY_SECONDS.clone()),
            Box::new(OTEL_EXPORTER_FAILURES_TOTAL.clone()),
            Box::new(OTEL_EXPORTER_RUNTIME_FAILURES_TOTAL.clone()),
            Box::new(OTEL_EXPORTER_DEGRADED.clone()),
        ] {
            METRICS_REGISTRY
                .register(collector)
                .expect("register a RyFrame metric");
        }

        if let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            PROCESS_START_TIME_SECONDS.set(elapsed.as_secs_f64());
        }
    });
}

pub fn begin_http_request() {
    ensure_registered();
    HTTP_REQUESTS_IN_FLIGHT.inc();
}

pub fn finish_http_request(method: &str, path: &str, status: u16, duration: Duration) {
    ensure_registered();
    HTTP_REQUESTS_IN_FLIGHT.dec();
    let status = status.to_string();
    HTTP_REQUESTS_TOTAL
        .with_label_values(&[method, path, &status])
        .inc();
    HTTP_REQUEST_DURATION
        .with_label_values(&[method, path])
        .observe(duration.as_secs_f64());
}

pub fn abandon_http_request() {
    ensure_registered();
    HTTP_REQUESTS_IN_FLIGHT.dec();
}

pub fn metrics_text() -> String {
    ensure_registered();
    let mut buffer = Vec::new();
    if let Err(error) = TextEncoder::new().encode(&METRICS_REGISTRY.gather(), &mut buffer) {
        tracing::error!(%error, "failed to encode Prometheus metrics");
        return String::new();
    }
    String::from_utf8(buffer).unwrap_or_default()
}

pub fn record_refresh_replay() {
    ensure_registered();
    AUTH_REFRESH_REPLAY_TOTAL.inc();
}

pub fn record_csrf_rejection() {
    ensure_registered();
    AUTH_CSRF_REJECTED_TOTAL.inc();
}

pub fn record_redis_degraded(subsystem: &str) {
    ensure_registered();
    REDIS_DEGRADED_TOTAL.with_label_values(&[subsystem]).inc();
}

pub fn set_redis_degraded_state(subsystem: &str, degraded: bool) {
    ensure_registered();
    REDIS_DEGRADED_STATE
        .with_label_values(&[subsystem])
        .set(i64::from(degraded));
}

pub fn record_idempotency_conflict(reason: &str) {
    ensure_registered();
    IDEMPOTENCY_CONFLICTS_TOTAL
        .with_label_values(&[reason])
        .inc();
}

pub fn record_rate_limit_rejection(scope: &str) {
    ensure_registered();
    RATE_LIMIT_REJECTIONS_TOTAL
        .with_label_values(&[scope])
        .inc();
}

pub fn record_readiness_failure(dependency: &str) {
    ensure_registered();
    READINESS_FAILURES_TOTAL
        .with_label_values(&[dependency])
        .inc();
}

/// 记录一次操作审计失败；阶段值必须由调用方使用固定常量。
pub fn record_audit_failure(stage: &'static str) {
    ensure_registered();
    AUDIT_FAILURES_TOTAL.with_label_values(&[stage]).inc();
}

/// 记录一次 WebSocket 票据申请、消费或拒绝结果。
pub fn record_ws_ticket(result: &str) {
    ensure_registered();
    WS_TICKETS_TOTAL.with_label_values(&[result]).inc();
}

/// 设置当前进程中的认证消息 WebSocket 连接数。
pub fn set_ws_connections(connections: usize) {
    ensure_registered();
    WS_CONNECTIONS.set(connections as i64);
}

/// 记录消息实时投递结果，不携带租户或用户等高基数标签。
pub fn record_message_delivery(result: &str) {
    ensure_registered();
    MESSAGE_DELIVERY_TOTAL.with_label_values(&[result]).inc();
}

/// 设置当前进程的消息 Redis 监听连接状态。
pub fn set_message_redis_listener_connected(connected: bool) {
    ensure_registered();
    MESSAGE_REDIS_LISTENER_CONNECTED.set(i64::from(connected));
}

/// 记录共享补拉调度器的一次身份查询，不携带租户、用户或连接标签。
pub fn record_message_replay_query(result: &'static str) {
    ensure_registered();
    MESSAGE_REPLAY_QUERY_TOTAL
        .with_label_values(&[result])
        .inc();
}

/// 累加消息保留任务删除的到期记录数。
pub fn record_message_retention_deleted(count: u64) {
    ensure_registered();
    MESSAGE_RETENTION_DELETED_TOTAL.inc_by(count);
}

/// 设置数据库节点的当前路由健康状态；节点名称必须来自稳定的部署配置。
pub fn set_database_node_health(name: &str, kind: &'static str, healthy: bool) {
    ensure_registered();
    DB_NODE_UP
        .with_label_values(&[name, kind])
        .set(i64::from(healthy));
}

/// 记录一次数据库读路由选择；标签值仅允许使用调用方定义的有限集合。
pub fn record_database_read_selection(target: &'static str, reason: &'static str) {
    ensure_registered();
    DB_READ_SELECTION_TOTAL
        .with_label_values(&[target, reason])
        .inc();
}

/// 记录最终一致性读取因缺少健康副本而回退主库的次数。
pub fn record_database_read_fallback() {
    ensure_registered();
    DB_READ_FALLBACK_TOTAL.inc();
}

/// 返回当前进程累计的最终一致性读取回退次数。
pub fn database_read_fallback_total() -> u64 {
    ensure_registered();
    DB_READ_FALLBACK_TOTAL.get()
}

/// 返回当前进程按固定目标与原因统计的数据库读路由次数。
pub fn database_read_selection_totals() -> Vec<(&'static str, &'static str, u64)> {
    ensure_registered();
    [
        ("primary", "strong"),
        ("replica", "replica"),
        ("primary", "fallback"),
    ]
    .into_iter()
    .map(|(target, reason)| {
        let count = DB_READ_SELECTION_TOTAL
            .with_label_values(&[target, reason])
            .get();
        (target, reason, count)
    })
    .collect()
}

/// 重置租户数据低基数聚合，调用方随后写入本次完整快照。
pub fn reset_tenant_data_aggregates() {
    ensure_registered();
    TENANT_DATA_TARGETS.reset();
    TENANT_DATA_PLACEMENTS.reset();
}

pub fn set_tenant_data_target_count(mode: &'static str, health: &'static str, count: usize) {
    ensure_registered();
    TENANT_DATA_TARGETS
        .with_label_values(&[mode, health])
        .set(count as i64);
}

pub fn set_tenant_data_placement_count(mode: &'static str, state: &'static str, count: u64) {
    ensure_registered();
    TENANT_DATA_PLACEMENTS
        .with_label_values(&[mode, state])
        .set(count as i64);
}

pub fn set_tenant_data_pool_stats(
    open: usize,
    opening: usize,
    reserved_connections: u32,
    active_leases: usize,
) {
    ensure_registered();
    TENANT_DATA_POOL_OPEN.set(open as i64);
    TENANT_DATA_POOL_OPENING.set(opening as i64);
    TENANT_DATA_POOL_RESERVED_CONNECTIONS.set(reserved_connections as i64);
    TENANT_DATA_POOL_ACTIVE_LEASES.set(active_leases as i64);
}

/// 设置已注册任务类型在指定状态下的队列深度。
pub fn set_job_queue_depth(job_type: &str, status: &'static str, depth: u64) {
    ensure_registered();
    JOB_QUEUE_DEPTH
        .with_label_values(&[job_type, status])
        .set(depth as i64);
}

/// 设置已就绪任务的最大等待年龄；零表示该类型当前没有可执行任务。
pub fn set_job_oldest_ready_age(job_type: &str, age: Duration) {
    ensure_registered();
    JOB_OLDEST_READY_AGE_SECONDS
        .with_label_values(&[job_type])
        .set(age.as_secs() as i64);
}

/// 记录后台任务从领取到状态落库完成的执行时长。
pub fn observe_job_duration(job_type: &str, result: &'static str, duration: Duration) {
    ensure_registered();
    JOB_DURATION_SECONDS
        .with_label_values(&[job_type, result])
        .observe(duration.as_secs_f64());
}

/// 记录一次后台任务或 Outbox 的领取尝试。
pub fn record_job_claim_attempt(queue: &'static str, result: &'static str) {
    ensure_registered();
    JOB_CLAIM_ATTEMPTS_TOTAL
        .with_label_values(&[queue, result])
        .inc();
}

/// 记录一次本地或 Redis 队列唤醒提示的结果。
pub fn record_job_wakeup(queue: &'static str, transport: &'static str, result: &'static str) {
    ensure_registered();
    JOB_WAKEUP_TOTAL
        .with_label_values(&[queue, transport, result])
        .inc();
}

/// 设置本进程 Redis 队列唤醒订阅连接状态。
pub fn set_job_wakeup_listener_up(queue: &'static str, up: bool) {
    ensure_registered();
    JOB_WAKEUP_LISTENER_UP
        .with_label_values(&[queue])
        .set(i64::from(up));
}

/// 记录一个被忽略的 Redis 队列唤醒协议负载。
pub fn record_job_wakeup_protocol_error(result: &'static str) {
    ensure_registered();
    JOB_WAKEUP_PROTOCOL_ERRORS_TOTAL
        .with_label_values(&[result])
        .inc();
}

/// 记录数据库调度扫描结果。
pub fn record_job_schedule_scan(result: &'static str) {
    ensure_registered();
    JOB_SCHEDULE_SCAN_TOTAL.with_label_values(&[result]).inc();
}

/// 记录低基数调度触发结果。
pub fn record_job_schedule_trigger(outcome: &'static str) {
    ensure_registered();
    JOB_SCHEDULE_TRIGGER_TOTAL
        .with_label_values(&[outcome])
        .inc();
}

/// 记录调度领取相对计划时间的延迟。
pub fn observe_job_schedule_lag(lag: Duration) {
    ensure_registered();
    JOB_SCHEDULE_LAG_SECONDS.observe(lag.as_secs_f64());
}

/// 记录一次授权缓存读取结果；范围和结果均由调用方使用固定枚举。
pub fn record_authorization_cache_lookup(scope: &'static str, result: &'static str) {
    ensure_registered();
    AUTHORIZATION_CACHE_LOOKUPS_TOTAL
        .with_label_values(&[scope, result])
        .inc();
}

/// 记录一次成功的消息确认操作耗时。
pub fn observe_message_ack_latency(duration: Duration) {
    ensure_registered();
    MESSAGE_ACK_LATENCY_SECONDS.observe(duration.as_secs_f64());
}

/// 记录 OTLP 导出器初始化失败并将其标记为降级。
pub fn record_otel_exporter_failure() {
    ensure_registered();
    OTEL_EXPORTER_FAILURES_TOTAL.inc();
    OTEL_EXPORTER_DEGRADED.set(1);
}

/// 记录 OTLP 导出器在运行或关闭期间发生的失败。
///
/// 初始化降级状态由独立指标表达；运行期瞬态失败仅累加计数，避免将一次可恢复
/// 的网络抖动错误地固化为进程级降级状态。
pub fn record_otel_exporter_runtime_failure() {
    ensure_registered();
    OTEL_EXPORTER_RUNTIME_FAILURES_TOTAL.inc();
}

/// 设置 OTLP 导出器的当前降级状态。
pub fn set_otel_exporter_degraded(degraded: bool) {
    ensure_registered();
    OTEL_EXPORTER_DEGRADED.set(i64::from(degraded));
}

pub fn spawn_process_metrics_updater() {
    ensure_registered();
    tokio::spawn(async {
        let mut system = sysinfo::System::new_all();
        let pid = sysinfo::Pid::from_u32(std::process::id());

        loop {
            system.refresh_all();
            if let Some(process) = system.process(pid) {
                // sysinfo 以 CPU 毫秒为单位提供累计 CPU 时间。
                PROCESS_CPU_SECONDS.set(process.accumulated_cpu_time() as f64 / 1000.0);
                // 自 sysinfo 0.30 起，进程内存已按字节报告。
                PROCESS_RESIDENT_MEMORY_BYTES.set(process.memory() as f64);
                PROCESS_VIRTUAL_MEMORY_BYTES.set(process.virtual_memory() as f64);

                #[cfg(target_os = "linux")]
                if let Ok(status) = std::fs::read_to_string("/proc/self/status")
                    && let Some(thread_count) = status
                        .lines()
                        .find(|line| line.starts_with("Threads:"))
                        .and_then(|line| line.split_whitespace().nth(1))
                        .and_then(|value| value.parse::<f64>().ok())
                {
                    PROCESS_THREADS.set(thread_count);
                }

                #[cfg(target_os = "linux")]
                if let Ok(entries) = std::fs::read_dir("/proc/self/fd") {
                    PROCESS_OPEN_FDS.set(entries.count() as f64);
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}
