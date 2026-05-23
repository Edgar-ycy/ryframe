/// 请求日志中间件
///
/// 记录每个 HTTP 请求的 method + path + status + latency。
pub fn request_log_layer() -> tower_http::trace::TraceLayer<
    tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>,
> {
    tower_http::trace::TraceLayer::new_for_http()
}
