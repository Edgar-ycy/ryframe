use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

pub type DatabaseReadSelections = Vec<(&'static str, &'static str, u64)>;

#[derive(Clone, Copy)]
pub struct ApiMetricsHooks {
    pub begin_http_request: fn(),
    pub finish_http_request: fn(&str, &str, u16, Duration),
    pub abandon_http_request: fn(),
    pub metrics_text: fn() -> String,
    pub record_refresh_replay: fn(),
    pub record_csrf_rejection: fn(),
    pub record_redis_degraded: fn(&str),
    pub record_idempotency_conflict: fn(&str),
    pub record_rate_limit_rejection: fn(&str),
    pub record_ws_ticket: fn(&str),
    pub set_ws_connections: fn(usize),
    pub record_message_delivery: fn(&str),
    pub set_message_redis_listener_connected: fn(bool),
    pub record_message_replay_query: fn(&'static str),
    pub database_read_fallback_total: fn() -> u64,
    pub database_read_selection_totals: fn() -> DatabaseReadSelections,
    pub observe_message_ack_latency: fn(Duration),
}

static HOOKS: OnceLock<ApiMetricsHooks> = OnceLock::new();

pub fn install(hooks: ApiMetricsHooks) -> Result<(), &'static str> {
    HOOKS
        .set(hooks)
        .map_err(|_| "API 指标钩子已经初始化，不能重复安装")
}

/// 一次 HTTP 请求的低基数指标观察。
pub struct HttpRequestObservation {
    method: String,
    path: String,
    started: Instant,
    active: bool,
}

impl HttpRequestObservation {
    pub fn start(method: String, path: String) -> Self {
        let active = HOOKS.get().is_some();
        if let Some(hooks) = HOOKS.get() {
            (hooks.begin_http_request)();
        }
        Self {
            method,
            path,
            started: Instant::now(),
            active,
        }
    }

    pub fn finish(mut self, status: u16) {
        if let Some(hooks) = HOOKS.get().filter(|_| self.active) {
            (hooks.finish_http_request)(&self.method, &self.path, status, self.started.elapsed());
        }
        self.active = false;
    }
}

impl Drop for HttpRequestObservation {
    fn drop(&mut self) {
        if let Some(hooks) = HOOKS.get().filter(|_| self.active) {
            (hooks.abandon_http_request)();
        }
    }
}

pub fn metrics_text() -> String {
    HOOKS
        .get()
        .map_or_else(String::new, |hooks| (hooks.metrics_text)())
}

pub fn record_refresh_replay() {
    if let Some(hooks) = HOOKS.get() {
        (hooks.record_refresh_replay)();
    }
}

pub fn record_csrf_rejection() {
    if let Some(hooks) = HOOKS.get() {
        (hooks.record_csrf_rejection)();
    }
}

pub fn record_redis_degraded(subsystem: &str) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.record_redis_degraded)(subsystem);
    }
}

pub fn record_idempotency_conflict(reason: &str) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.record_idempotency_conflict)(reason);
    }
}

pub fn record_rate_limit_rejection(scope: &str) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.record_rate_limit_rejection)(scope);
    }
}

pub fn record_ws_ticket(result: &str) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.record_ws_ticket)(result);
    }
}

pub fn set_ws_connections(connections: usize) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.set_ws_connections)(connections);
    }
}

pub fn record_message_delivery(result: &str) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.record_message_delivery)(result);
    }
}

pub fn set_message_redis_listener_connected(connected: bool) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.set_message_redis_listener_connected)(connected);
    }
}

pub fn record_message_replay_query(result: &'static str) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.record_message_replay_query)(result);
    }
}

pub fn database_read_fallback_total() -> u64 {
    HOOKS
        .get()
        .map_or(0, |hooks| (hooks.database_read_fallback_total)())
}

pub fn database_read_selection_totals() -> DatabaseReadSelections {
    HOOKS
        .get()
        .map_or_else(Vec::new, |hooks| (hooks.database_read_selection_totals)())
}

pub fn observe_message_ack_latency(duration: Duration) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.observe_message_ack_latency)(duration);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{ApiMetricsHooks, HttpRequestObservation};

    static STARTED: AtomicUsize = AtomicUsize::new(0);
    static FINISHED: AtomicUsize = AtomicUsize::new(0);
    static ABANDONED: AtomicUsize = AtomicUsize::new(0);

    fn increment_started() {
        STARTED.fetch_add(1, Ordering::Relaxed);
    }

    fn increment_finished(_: &str, _: &str, _: u16, _: std::time::Duration) {
        FINISHED.fetch_add(1, Ordering::Relaxed);
    }

    fn increment_abandoned() {
        ABANDONED.fetch_add(1, Ordering::Relaxed);
    }

    fn noop() {}
    fn noop_str(_: &str) {}
    fn noop_static_str(_: &'static str) {}
    fn noop_usize(_: usize) {}
    fn noop_bool(_: bool) {}
    fn noop_duration(_: std::time::Duration) {}
    fn empty_text() -> String {
        String::new()
    }
    fn zero() -> u64 {
        0
    }
    fn empty_selections() -> super::DatabaseReadSelections {
        Vec::new()
    }

    #[test]
    fn observation_finishes_or_abandons_exactly_once() {
        super::install(ApiMetricsHooks {
            begin_http_request: increment_started,
            finish_http_request: increment_finished,
            abandon_http_request: increment_abandoned,
            metrics_text: empty_text,
            record_refresh_replay: noop,
            record_csrf_rejection: noop,
            record_redis_degraded: noop_str,
            record_idempotency_conflict: noop_str,
            record_rate_limit_rejection: noop_str,
            record_ws_ticket: noop_str,
            set_ws_connections: noop_usize,
            record_message_delivery: noop_str,
            set_message_redis_listener_connected: noop_bool,
            record_message_replay_query: noop_static_str,
            database_read_fallback_total: zero,
            database_read_selection_totals: empty_selections,
            observe_message_ack_latency: noop_duration,
        })
        .expect("测试指标钩子只能安装一次");

        HttpRequestObservation::start("GET".into(), "/readyz".into()).finish(200);
        drop(HttpRequestObservation::start(
            "POST".into(),
            "/cancelled".into(),
        ));

        assert_eq!(STARTED.load(Ordering::Relaxed), 2);
        assert_eq!(FINISHED.load(Ordering::Relaxed), 1);
        assert_eq!(ABANDONED.load(Ordering::Relaxed), 1);
    }
}
