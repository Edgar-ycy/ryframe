//! 后台任务的 W3C Trace Context 应用端口。

use std::sync::{Arc, OnceLock};

use ryframe_kernel::{AppError, AppResult};

/// 可跨进程持久化的完整 W3C Trace Context。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PersistedTraceContext {
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

/// 读取当前追踪上下文，并为消费端 Span 恢复远端父上下文。
pub trait TraceContextPort: Send + Sync {
    fn current(&self) -> PersistedTraceContext;

    fn set_parent(&self, span: &tracing::Span, traceparent: Option<&str>, tracestate: Option<&str>);
}

static TRACE_CONTEXT: OnceLock<Arc<dyn TraceContextPort>> = OnceLock::new();

pub fn install_trace_context_port(port: Arc<dyn TraceContextPort>) -> AppResult<()> {
    TRACE_CONTEXT
        .set(port)
        .map_err(|_| AppError::Config("链路追踪上下文端口不能重复安装".into()))
}

pub(crate) fn current_trace_context() -> PersistedTraceContext {
    TRACE_CONTEXT
        .get()
        .map_or_else(PersistedTraceContext::default, |port| port.current())
}

pub(crate) fn set_parent(
    span: &tracing::Span,
    traceparent: Option<&str>,
    tracestate: Option<&str>,
) {
    if let Some(port) = TRACE_CONTEXT.get() {
        port.set_parent(span, traceparent, tracestate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_adapter_fails_closed_to_empty_context() {
        assert_eq!(current_trace_context(), PersistedTraceContext::default());
    }
}
