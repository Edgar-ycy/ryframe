//! 后台任务的 W3C Trace Context 传递辅助。

use std::collections::BTreeMap;

use opentelemetry::{
    Context, global,
    propagation::{Extractor, Injector},
};

/// 可跨进程持久化的完整 W3C Trace Context。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PersistedTraceContext {
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

/// 从 tracing-opentelemetry 激活的当前上下文取出可持久化的 W3C Trace Context。
pub(crate) fn current_trace_context() -> PersistedTraceContext {
    let context = Context::current();
    let mut carrier = HeaderCarrier::default();
    global::get_text_map_propagator(|propagator| propagator.inject_context(&context, &mut carrier));
    PersistedTraceContext {
        traceparent: carrier.0.remove("traceparent"),
        tracestate: carrier.0.remove("tracestate"),
    }
}

/// 将任务中保存的 W3C Trace Context 解析为远端父上下文。
pub(crate) fn extract_parent_context(
    traceparent: Option<&str>,
    tracestate: Option<&str>,
) -> Context {
    let mut values = BTreeMap::new();
    if let Some(traceparent) = traceparent.filter(|value| !value.trim().is_empty()) {
        values.insert("traceparent".to_owned(), traceparent.to_owned());
    }
    if let Some(tracestate) = tracestate.filter(|value| !value.trim().is_empty()) {
        values.insert("tracestate".to_owned(), tracestate.to_owned());
    }
    let carrier = HeaderCarrier(values);
    global::get_text_map_propagator(|propagator| propagator.extract(&carrier))
}

#[derive(Default)]
struct HeaderCarrier(BTreeMap<String, String>);

impl Injector for HeaderCarrier {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_ascii_lowercase(), value);
    }
}

impl Extractor for HeaderCarrier {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(&key.to_ascii_lowercase()).map(String::as_str)
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{current_trace_context, extract_parent_context};
    use opentelemetry::{
        global,
        trace::{TraceContextExt, TracerProvider as _},
    };
    use opentelemetry_sdk::{
        propagation::TraceContextPropagator,
        trace::{InMemorySpanExporter, SdkTracerProvider},
    };
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::layer::SubscriberExt;

    fn install_trace_context_propagator() {
        global::set_text_map_propagator(TraceContextPropagator::new());
    }

    #[test]
    fn invalid_traceparent_falls_back_to_a_valid_context() {
        install_trace_context_propagator();
        let context = extract_parent_context(Some("not-a-traceparent"), Some("vendor=value"));
        assert!(!context.has_active_span());
    }

    #[test]
    fn valid_trace_context_restores_the_remote_parent_identity_and_state() {
        install_trace_context_propagator();
        let context = extract_parent_context(
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
            Some("vendor=value"),
        );
        let span = context.span();
        let span_context = span.span_context();

        assert!(span_context.is_valid());
        assert!(span_context.is_remote());
        assert_eq!(
            span_context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(span_context.span_id().to_string(), "00f067aa0ba902b7");
        assert_eq!(span_context.trace_state().header(), "vendor=value");
    }

    #[test]
    fn current_context_round_trips_into_a_worker_child_span() {
        install_trace_context_propagator();
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        global::set_tracer_provider(provider.clone());
        let subscriber = tracing_subscriber::registry().with(
            tracing_opentelemetry::layer()
                .with_tracer(provider.tracer("ryframe-trace-context-test")),
        );

        tracing::subscriber::with_default(subscriber, || {
            let api = tracing::info_span!("HTTP");
            api.set_parent(extract_parent_context(
                Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
                Some("vendor=value"),
            ))
            .expect("应能设置 API 外部父上下文");
            let persisted = api.in_scope(current_trace_context);
            assert!(persisted.traceparent.is_some());
            assert_eq!(persisted.tracestate.as_deref(), Some("vendor=value"));

            let worker = tracing::info_span!("background_job");
            worker
                .set_parent(extract_parent_context(
                    persisted.traceparent.as_deref(),
                    persisted.tracestate.as_deref(),
                ))
                .expect("应能恢复 Worker 父上下文");
            worker.in_scope(|| {});
        });
        provider.force_flush().expect("应能刷新内存导出器");

        let spans = exporter
            .get_finished_spans()
            .expect("应能读取已导出的 span");
        let api = spans
            .iter()
            .find(|span| span.name == "HTTP")
            .expect("应导出 API span");
        let worker = spans
            .iter()
            .find(|span| span.name == "background_job")
            .expect("应导出 Worker span");
        assert_eq!(
            api.span_context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(worker.span_context.trace_id(), api.span_context.trace_id());
        assert_eq!(worker.parent_span_id, api.span_context.span_id());
        assert_eq!(worker.span_context.trace_state().header(), "vendor=value");
    }
}
