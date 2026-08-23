use lazy_static::lazy_static;
use prometheus::{
    Gauge, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge,
    IntGaugeVec, Opts,
};

lazy_static! {
    pub(super) static ref HTTP_REQUESTS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "http_requests_total",
            "HTTP requests by method, path, and status"
        ),
        &["method", "path", "status"],
    )
    .expect("create http_requests_total");
    pub(super) static ref HTTP_REQUEST_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("http_request_duration_seconds", "HTTP request latency").buckets(vec![
            0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0
        ]),
        &["method", "path"],
    )
    .expect("create http_request_duration_seconds");
    pub(super) static ref HTTP_REQUESTS_IN_FLIGHT: IntGauge = IntGauge::new(
        "http_requests_in_flight",
        "HTTP requests currently being handled",
    )
    .expect("create http_requests_in_flight");
    pub(super) static ref PROCESS_CPU_SECONDS: Gauge = Gauge::new(
        "process_cpu_seconds_total",
        "Total process CPU time in seconds",
    )
    .expect("create process_cpu_seconds_total");
    pub(super) static ref PROCESS_RESIDENT_MEMORY_BYTES: Gauge = Gauge::new(
        "process_resident_memory_bytes",
        "Resident process memory in bytes",
    )
    .expect("create process_resident_memory_bytes");
    pub(super) static ref PROCESS_VIRTUAL_MEMORY_BYTES: Gauge = Gauge::new(
        "process_virtual_memory_bytes",
        "Virtual process memory in bytes",
    )
    .expect("create process_virtual_memory_bytes");
    pub(super) static ref PROCESS_OPEN_FDS: Gauge =
        Gauge::new("process_open_fds", "Open process file descriptors")
            .expect("create process_open_fds");
    pub(super) static ref PROCESS_THREADS: Gauge =
        Gauge::new("process_threads", "Process thread count").expect("create process_threads");
    pub(super) static ref PROCESS_START_TIME_SECONDS: Gauge = Gauge::new(
        "process_start_time_seconds",
        "Process start time as a Unix timestamp",
    )
    .expect("create process_start_time_seconds");
    pub(super) static ref AUTH_REFRESH_REPLAY_TOTAL: IntCounter = IntCounter::new(
        "auth_refresh_replay_total",
        "Confirmed refresh-token replay attempts",
    )
    .expect("create auth_refresh_replay_total");
    pub(super) static ref AUTH_CSRF_REJECTED_TOTAL: IntCounter = IntCounter::new(
        "auth_csrf_rejected_total",
        "Authentication requests rejected by CSRF validation",
    )
    .expect("create auth_csrf_rejected_total");
    pub(super) static ref REDIS_DEGRADED_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "redis_degraded_total",
            "Redis degradation events by subsystem"
        ),
        &["subsystem"],
    )
    .expect("create redis_degraded_total");
    pub(super) static ref REDIS_DEGRADED_STATE: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "redis_degraded_state",
            "Current Redis degradation state by subsystem"
        ),
        &["subsystem"],
    )
    .expect("create redis_degraded_state");
    pub(super) static ref IDEMPOTENCY_CONFLICTS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "idempotency_conflicts_total",
            "Idempotency conflicts by reason"
        ),
        &["reason"],
    )
    .expect("create idempotency_conflicts_total");
    pub(super) static ref RATE_LIMIT_REJECTIONS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "rate_limit_rejections_total",
            "Rate-limit rejections by scope"
        ),
        &["scope"],
    )
    .expect("create rate_limit_rejections_total");
    pub(super) static ref READINESS_FAILURES_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "readiness_failures_total",
            "Readiness failures by dependency"
        ),
        &["dependency"],
    )
    .expect("create readiness_failures_total");
    pub(super) static ref CONNECTOR_OPERATIONS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "connector_operations_total",
            "Outbound connector operations by bounded connector, operation, and result",
        ),
        &["connector", "operation", "result"],
    )
    .expect("create connector_operations_total");
    pub(super) static ref CONNECTOR_OPERATION_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new(
            "connector_operation_duration_seconds",
            "Outbound connector latency by bounded connector and operation",
        )
        .buckets(vec![
            0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
            120.0, 300.0,
        ]),
        &["connector", "operation"],
    )
    .expect("create connector_operation_duration_seconds");
    pub(super) static ref AUDIT_FAILURES_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "audit_failures_total",
            "Operation-audit failures by bounded processing stage"
        ),
        &["stage"],
    )
    .expect("create audit_failures_total");
    pub(super) static ref WS_CONNECTIONS: IntGauge = IntGauge::new(
        "ws_connections",
        "Current authenticated message WebSocket connections",
    )
    .expect("create ws_connections");
    pub(super) static ref WS_TICKETS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new("ws_ticket_total", "WebSocket ticket outcomes by result",),
        &["result"],
    )
    .expect("create ws_ticket_total");
    pub(super) static ref MESSAGE_DELIVERY_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "message_delivery_total",
            "Message WebSocket delivery outcomes by result",
        ),
        &["result"],
    )
    .expect("create message_delivery_total");
    pub(super) static ref MESSAGE_REDIS_LISTENER_CONNECTED: IntGauge = IntGauge::new(
        "message_redis_listener_connected",
        "Whether the message Redis listener is currently connected",
    )
    .expect("create message_redis_listener_connected");
    pub(super) static ref MESSAGE_REPLAY_QUERY_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "message_replay_query_total",
            "Shared inbox replay queries by bounded result",
        ),
        &["result"],
    )
    .expect("create message_replay_query_total");
    pub(super) static ref MESSAGE_RETENTION_DELETED_TOTAL: IntCounter = IntCounter::new(
        "message_retention_deleted_total",
        "Expired message records removed by retention jobs",
    )
    .expect("create message_retention_deleted_total");
    pub(super) static ref DB_NODE_UP: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "db_node_up",
            "Database node routing health by configured node name and kind",
        ),
        &["name", "kind"],
    )
    .expect("create db_node_up");
    pub(super) static ref DB_READ_SELECTION_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "db_read_selection_total",
            "Database read selections by target and bounded reason",
        ),
        &["target", "reason"],
    )
    .expect("create db_read_selection_total");
    pub(super) static ref DB_READ_FALLBACK_TOTAL: IntCounter = IntCounter::new(
        "db_read_fallback_total",
        "Eventual-consistency reads routed to primary because no healthy replica was available",
    )
    .expect("create db_read_fallback_total");
    pub(super) static ref TENANT_DATA_TARGETS: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "tenant_data_targets",
            "Tenant-data target count by bounded mode and cached health",
        ),
        &["mode", "health"],
    )
    .expect("create tenant_data_targets");
    pub(super) static ref TENANT_DATA_PLACEMENTS: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "tenant_data_placements",
            "Tenant-data placement count by bounded state and target mode",
        ),
        &["mode", "state"],
    )
    .expect("create tenant_data_placements");
    pub(super) static ref TENANT_DATA_POOL_OPEN: IntGauge = IntGauge::new(
        "tenant_data_pool_open",
        "Open tenant-data MySQL pools in this process",
    )
    .expect("create tenant_data_pool_open");
    pub(super) static ref TENANT_DATA_POOL_OPENING: IntGauge = IntGauge::new(
        "tenant_data_pool_opening",
        "Tenant-data MySQL pools currently opening in this process",
    )
    .expect("create tenant_data_pool_opening");
    pub(super) static ref TENANT_DATA_POOL_RESERVED_CONNECTIONS: IntGauge = IntGauge::new(
        "tenant_data_pool_reserved_connections",
        "Connections atomically reserved across tenant-data pools",
    )
    .expect("create tenant_data_pool_reserved_connections");
    pub(super) static ref TENANT_DATA_POOL_ACTIVE_LEASES: IntGauge = IntGauge::new(
        "tenant_data_pool_active_leases",
        "Active tenant-data pool leases in this process",
    )
    .expect("create tenant_data_pool_active_leases");
    pub(super) static ref JOB_QUEUE_DEPTH: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "job_queue_depth",
            "Persistent background-job counts by registered type and status",
        ),
        &["type", "status"],
    )
    .expect("create job_queue_depth");
    pub(super) static ref JOB_OLDEST_READY_AGE_SECONDS: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "job_oldest_ready_age_seconds",
            "Age of the oldest ready background job by registered type",
        ),
        &["type"],
    )
    .expect("create job_oldest_ready_age_seconds");
    pub(super) static ref JOB_DURATION_SECONDS: HistogramVec = HistogramVec::new(
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
    pub(super) static ref JOB_CLAIM_ATTEMPTS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "job_claim_attempts_total",
            "Persistent queue claim attempts by queue and bounded result",
        ),
        &["queue", "result"],
    )
    .expect("create job_claim_attempts_total");
    pub(super) static ref JOB_WAKEUP_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "job_wakeup_total",
            "Local and Redis queue wakeup outcomes by bounded transport and result",
        ),
        &["queue", "transport", "result"],
    )
    .expect("create job_wakeup_total");
    pub(super) static ref JOB_WAKEUP_LISTENER_UP: IntGaugeVec = IntGaugeVec::new(
        Opts::new(
            "job_wakeup_listener_up",
            "Whether the current process Redis wakeup listener is connected",
        ),
        &["queue"],
    )
    .expect("create job_wakeup_listener_up");
    pub(super) static ref JOB_WAKEUP_PROTOCOL_ERRORS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "job_wakeup_protocol_errors_total",
            "Ignored Redis wakeup payloads by bounded validation result",
        ),
        &["result"],
    )
    .expect("create job_wakeup_protocol_errors_total");
    pub(super) static ref JOB_SCHEDULE_SCAN_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "job_schedule_scan_total",
            "Database-backed schedule scans by bounded result",
        ),
        &["result"],
    )
    .expect("create job_schedule_scan_total");
    pub(super) static ref JOB_SCHEDULE_TRIGGER_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "job_schedule_trigger_total",
            "Schedule trigger attempts by bounded outcome",
        ),
        &["outcome"],
    )
    .expect("create job_schedule_trigger_total");
    pub(super) static ref JOB_SCHEDULE_LAG_SECONDS: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "job_schedule_lag_seconds",
            "Delay between a scheduled UTC fire time and database claim time",
        )
        .buckets(vec![
            0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 15.0, 60.0, 300.0
        ]),
    )
    .expect("create job_schedule_lag_seconds");
    pub(super) static ref AUTHORIZATION_CACHE_LOOKUPS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new(
            "authorization_cache_lookups_total",
            "Authorization cache lookup outcomes by bounded scope and result",
        ),
        &["scope", "result"],
    )
    .expect("create authorization_cache_lookups_total");
    pub(super) static ref MESSAGE_ACK_LATENCY_SECONDS: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "message_ack_latency_seconds",
            "Successful message acknowledgement operation latency",
        )
        .buckets(vec![
            0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0
        ]),
    )
    .expect("create message_ack_latency_seconds");
    pub(super) static ref OTEL_EXPORTER_FAILURES_TOTAL: IntCounter = IntCounter::new(
        "otel_exporter_failures_total",
        "OpenTelemetry exporter initialization failures",
    )
    .expect("create otel_exporter_failures_total");
    pub(super) static ref OTEL_EXPORTER_RUNTIME_FAILURES_TOTAL: IntCounter = IntCounter::new(
        "otel_exporter_runtime_failures_total",
        "OpenTelemetry exporter runtime failures",
    )
    .expect("create otel_exporter_runtime_failures_total");
    pub(super) static ref OTEL_EXPORTER_DEGRADED: IntGauge = IntGauge::new(
        "otel_exporter_degraded",
        "Whether OpenTelemetry exporting is degraded after initialization failure",
    )
    .expect("create otel_exporter_degraded");
    pub(super) static ref METRICS_REGISTERED: std::sync::Once = std::sync::Once::new();
}
