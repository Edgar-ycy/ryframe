//! Prometheus HTTP, process, and security metrics.

use std::{sync::LazyLock, time::Instant};

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use lazy_static::lazy_static;
use prometheus::{
    Encoder, Gauge, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec,
    Opts, Registry, TextEncoder,
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

pub async fn metrics_middleware(request: Request, next: Next) -> Response {
    ensure_registered();
    let method = request.method().to_string();
    let path = request.extensions().get::<MatchedPath>().map_or_else(
        || normalize_path(request.uri().path()),
        |path| path.as_str().to_owned(),
    );
    let started = Instant::now();

    HTTP_REQUESTS_IN_FLIGHT.inc();
    let response = next.run(request).await;
    HTTP_REQUESTS_IN_FLIGHT.dec();

    let elapsed = started.elapsed();
    let status = response.status().as_u16().to_string();
    HTTP_REQUESTS_TOTAL
        .with_label_values(&[&method, &path, &status])
        .inc();
    HTTP_REQUEST_DURATION
        .with_label_values(&[&method, &path])
        .observe(elapsed.as_secs_f64());

    response
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

pub fn normalize_path(path: &str) -> String {
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.is_empty() {
        return "/".to_string();
    }

    let normalized = segments
        .into_iter()
        .map(|segment| {
            if segment.len() == 36
                && segment
                    .chars()
                    .filter(|character| *character == '-')
                    .count()
                    == 4
            {
                ":uuid".to_string()
            } else if segment.chars().all(|character| character.is_ascii_digit()) {
                ":id".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>();
    format!("/{}", normalized.join("/"))
}

pub fn spawn_process_metrics_updater() {
    ensure_registered();
    tokio::spawn(async {
        let mut system = sysinfo::System::new_all();
        let pid = sysinfo::Pid::from_u32(std::process::id());

        loop {
            system.refresh_all();
            if let Some(process) = system.process(pid) {
                // sysinfo exposes accumulated CPU time in CPU-milliseconds.
                PROCESS_CPU_SECONDS.set(process.accumulated_cpu_time() as f64 / 1000.0);
                // Since sysinfo 0.30, process memory is already reported in bytes.
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
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}
