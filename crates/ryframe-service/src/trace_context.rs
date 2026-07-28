//! 后台任务的 W3C Trace Context 传递辅助。

use std::collections::BTreeMap;

use opentelemetry::{
    Context, global,
    propagation::{Extractor, Injector},
};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// 从当前 tracing span 取出可持久化的 `traceparent` 值。
pub(crate) fn current_traceparent() -> Option<String> {
    let context = tracing::Span::current().context();
    let mut carrier = HeaderCarrier::default();
    global::get_text_map_propagator(|propagator| propagator.inject_context(&context, &mut carrier));
    carrier.0.remove("traceparent")
}

/// 将任务中保存的 `traceparent` 解析为远端父上下文。
pub(crate) fn extract_parent_context(traceparent: Option<&str>) -> Context {
    let mut values = BTreeMap::new();
    if let Some(traceparent) = traceparent.filter(|value| !value.trim().is_empty()) {
        values.insert("traceparent".to_owned(), traceparent.to_owned());
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
    use super::extract_parent_context;
    use opentelemetry::{global, trace::TraceContextExt};
    use opentelemetry_sdk::propagation::TraceContextPropagator;

    fn install_trace_context_propagator() {
        global::set_text_map_propagator(TraceContextPropagator::new());
    }

    #[test]
    fn invalid_traceparent_falls_back_to_a_valid_context() {
        install_trace_context_propagator();
        let context = extract_parent_context(Some("not-a-traceparent"));
        assert!(!context.has_active_span());
    }

    #[test]
    fn valid_traceparent_restores_the_remote_parent_identity() {
        install_trace_context_propagator();
        let context = extract_parent_context(Some(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        ));
        let span = context.span();
        let span_context = span.span_context();

        assert!(span_context.is_valid());
        assert!(span_context.is_remote());
        assert_eq!(
            span_context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(span_context.span_id().to_string(), "00f067aa0ba902b7");
    }
}
