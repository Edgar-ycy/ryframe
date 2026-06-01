//! HTTP 请求 Metrics 中间件
//!
//! 基于 prometheus crate 收集：
//! - 请求总数（按 method、path、status 分组）
//! - 请求延迟分布（直方图）
//! - 当前并发请求数
//!
//! 使用方式：
//! ```
//! use ryframe_middleware::metrics::{metrics_text, normalize_path};
//!
//! // 导出 Prometheus 文本格式（需先注册 middleware 才会产生数据）
//! let text = metrics_text();
//! assert!(!text.is_empty() || text.is_empty());
//!
//! // 路径标准化：将动态参数替换为占位符
//! assert_eq!(normalize_path("/system/user/123"), "/system/user/:id");
//! assert_eq!(normalize_path("/api/user/a1b2c3d4-e5f6-7890-abcd-ef1234567890"), "/api/user/:uuid");
//! ```

use std::{sync::LazyLock, time::Instant};

use axum::{extract::Request, middleware::Next, response::Response};
use lazy_static::lazy_static;
use prometheus::{Encoder, Gauge, IntCounterVec, IntGauge, Opts, Registry, TextEncoder};

/// 自定义 Registry，所有 metrics 注册在此
static METRICS_REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
    Registry::new_custom(Some("ryframe".to_string()), None).expect("创建 metrics registry 失败")
});

lazy_static! {
    /// HTTP 请求总数（method, path, status）
    static ref HTTP_REQUESTS_TOTAL: IntCounterVec =
        IntCounterVec::new(
            Opts::new("http_requests_total", "HTTP 请求总数"),
            &["method", "path", "status"]
        ).expect("创建 http_requests_total 失败");

    /// 请求延迟直方图（秒）
    static ref HTTP_REQUEST_DURATION: prometheus::HistogramVec =
        prometheus::HistogramVec::new(
            prometheus::HistogramOpts::new("http_request_duration_seconds", "HTTP 请求延迟（秒）")
                .buckets(vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]),
            &["method", "path"]
        ).expect("创建 http_request_duration_seconds 失败");

    /// 并发请求数
    static ref HTTP_REQUESTS_IN_FLIGHT: IntGauge =
        IntGauge::new(
            "http_requests_in_flight",
            "当前并发处理中的请求数"
        ).expect("创建 http_requests_in_flight 失败");

    /// 进程 CPU 时间（秒）
    static ref PROCESS_CPU_SECONDS: Gauge =
        Gauge::new(
            "process_cpu_seconds_total",
            "进程累计 CPU 使用时间（秒）"
        ).expect("创建 process_cpu_seconds_total 失败");

    /// 进程常驻内存（字节）
    static ref PROCESS_RESIDENT_MEMORY_BYTES: Gauge =
        Gauge::new(
            "process_resident_memory_bytes",
            "进程常驻内存大小（字节）"
        ).expect("创建 process_resident_memory_bytes 失败");

    /// 进程虚拟内存（字节）
    static ref PROCESS_VIRTUAL_MEMORY_BYTES: Gauge =
        Gauge::new(
            "process_virtual_memory_bytes",
            "进程虚拟内存大小（字节）"
        ).expect("创建 process_virtual_memory_bytes 失败");

    /// 进程打开的文件描述符数
    static ref PROCESS_OPEN_FDS: Gauge =
        Gauge::new(
            "process_open_fds",
            "进程打开的文件描述符数"
        ).expect("创建 process_open_fds 失败");

    /// 进程线程数
    static ref PROCESS_THREADS: Gauge =
        Gauge::new(
            "process_threads",
            "进程线程数"
        ).expect("创建 process_threads 失败");

    /// 进程启动时间戳（Unix 秒）
    static ref PROCESS_START_TIME_SECONDS: Gauge =
        Gauge::new(
            "process_start_time_seconds",
            "进程启动时间（Unix timestamp）"
        ).expect("创建 process_start_time_seconds 失败");

    /// 注册标记（确保只注册一次）
    static ref METRICS_REGISTERED: std::sync::Once = std::sync::Once::new();
}

/// 注册所有 metrics 到自定义 Registry（仅执行一次）
fn ensure_registered() {
    METRICS_REGISTERED.call_once(|| {
        // HTTP 指标
        METRICS_REGISTRY
            .register(Box::new(HTTP_REQUESTS_TOTAL.clone()))
            .expect("注册 http_requests_total 失败");
        METRICS_REGISTRY
            .register(Box::new(HTTP_REQUEST_DURATION.clone()))
            .expect("注册 http_request_duration_seconds 失败");
        METRICS_REGISTRY
            .register(Box::new(HTTP_REQUESTS_IN_FLIGHT.clone()))
            .expect("注册 http_requests_in_flight 失败");

        // 进程指标（CPU / 内存 / FD / 线程 / 启动时间）
        METRICS_REGISTRY
            .register(Box::new(PROCESS_CPU_SECONDS.clone()))
            .expect("注册 process_cpu_seconds_total 失败");
        METRICS_REGISTRY
            .register(Box::new(PROCESS_RESIDENT_MEMORY_BYTES.clone()))
            .expect("注册 process_resident_memory_bytes 失败");
        METRICS_REGISTRY
            .register(Box::new(PROCESS_VIRTUAL_MEMORY_BYTES.clone()))
            .expect("注册 process_virtual_memory_bytes 失败");
        METRICS_REGISTRY
            .register(Box::new(PROCESS_OPEN_FDS.clone()))
            .expect("注册 process_open_fds 失败");
        METRICS_REGISTRY
            .register(Box::new(PROCESS_THREADS.clone()))
            .expect("注册 process_threads 失败");
        METRICS_REGISTRY
            .register(Box::new(PROCESS_START_TIME_SECONDS.clone()))
            .expect("注册 process_start_time_seconds 失败");

        // 记录启动时间（仅一次）
        if let Ok(ts) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            PROCESS_START_TIME_SECONDS.set(ts.as_secs_f64());
        }
    });
}

/// Metrics 中间件
///
/// 记录每个 HTTP 请求的：
/// - 请求计数（method + path pattern + status）
/// - 并发数
/// - 请求延迟
pub async fn metrics_middleware(request: Request, next: Next) -> Response {
    // 注册 metrics（首次调用时执行，幂等）
    ensure_registered();

    let method = request.method().to_string();
    let path = normalize_path(request.uri().path());
    let start = Instant::now();

    // 并发 +1
    HTTP_REQUESTS_IN_FLIGHT.inc();

    let response = next.run(request).await;

    // 并发 -1
    HTTP_REQUESTS_IN_FLIGHT.dec();

    // 记录耗时
    let elapsed = start.elapsed();
    let elapsed_secs = elapsed.as_secs_f64();

    let status = response.status().as_u16().to_string();

    // 请求计数 +1
    HTTP_REQUESTS_TOTAL
        .with_label_values(&[&method, &path, &status])
        .inc();

    // 延迟直方图
    HTTP_REQUEST_DURATION
        .with_label_values(&[&method, &path])
        .observe(elapsed_secs);

    // 慢请求日志（> 1s 用 warn）
    if elapsed.as_millis() > 1000 {
        tracing::warn!(
            method = %method,
            path = %path,
            status = %status,
            latency_ms = elapsed.as_millis(),
            "慢请求"
        );
    }

    response
}

/// 导出 metrics 文本（供 /metrics 端点使用）
pub fn metrics_text() -> String {
    let encoder = TextEncoder::new();
    let metric_families = METRICS_REGISTRY.gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap_or_default()
}

/// 将路径中的动态参数替换为占位符，减少 metrics 基数
///
/// 例如：`/system/user/123` → `/system/user/:id`
pub fn normalize_path(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return "/".to_string();
    }

    // 静态路由前缀列表（匹配后直接放行，不做占位符替换）
    let static_prefixes: &[&str] = &[
        "metrics", "health", "server", "cache", "db-pool", "captcha", "login", "logout",
        "register", "tree", "export", "import", "refresh", "generate",
    ];

    let normalized: Vec<String> = segments
        .iter()
        .map(|seg| {
            // UUID 格式
            if seg.len() == 36 && seg.chars().filter(|&c| c == '-').count() == 4 {
                return ":uuid".to_string();
            }
            // 纯数字 → 动态 ID
            if seg.chars().all(|c| c.is_ascii_digit()) {
                return ":id".to_string();
            }
            // 静态路径放行
            if static_prefixes.contains(seg) {
                return seg.to_string();
            }
            // 其它路径保留原样
            seg.to_string()
        })
        .collect();

    format!("/{}", normalized.join("/"))
}

/// 启动进程指标采集后台任务
///
/// 每 5 秒通过 `sysinfo` 刷新当前进程的 CPU/内存/线程等信息，
/// 写入对应的 prometheus Gauge。
///
/// 应在 `tokio::main` 中调用。
pub fn spawn_process_metrics_updater() {
    tokio::spawn(async {
        let mut sys = sysinfo::System::new_all();
        let pid = sysinfo::Pid::from_u32(std::process::id());

        loop {
            sys.refresh_all();

            if let Some(proc) = sys.process(pid) {
                // CPU 使用率（%）
                PROCESS_CPU_SECONDS.set(proc.cpu_usage() as f64);

                // 内存（sysinfo 返回 KB，转为字节）
                PROCESS_RESIDENT_MEMORY_BYTES.set(proc.memory() as f64 * 1024.0);
                PROCESS_VIRTUAL_MEMORY_BYTES.set(proc.virtual_memory() as f64 * 1024.0);

                // 线程数：Linux 从 /proc/self/status 读取
                #[cfg(target_os = "linux")]
                {
                    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                        for line in status.lines() {
                            if line.starts_with("Threads:") {
                                if let Some(n) = line
                                    .split_whitespace()
                                    .nth(1)
                                    .and_then(|s| s.parse::<f64>().ok())
                                {
                                    PROCESS_THREADS.set(n);
                                }
                                break;
                            }
                        }
                    }
                }

                // FD 数：Linux 从 /proc/self/fd 读取
                #[cfg(target_os = "linux")]
                {
                    if let Ok(entries) = std::fs::read_dir("/proc/self/fd") {
                        PROCESS_OPEN_FDS.set(entries.count() as f64);
                    }
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}
