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
