use std::time::{Duration, SystemTime};

use opentelemetry::{
    Context as OtelContext, KeyValue, global,
    trace::{Span as _, SpanKind, Tracer as _},
};
use opentelemetry_semantic_conventions::attribute::{DB_OPERATION_NAME, DB_SYSTEM_NAME};
use tracing::Event;
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use super::fields::{SqlxEventFields, extract_sql_operation};

/// 为 SQLx 查询补充不包含原始 SQL 的 OpenTelemetry 数据库 Span。
pub struct DbSpanLayer;

impl DbSpanLayer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DbSpanLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for DbSpanLayer
where
    S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }

        let fields = SqlxEventFields::from_event(event);
        emit_db_span(
            extract_sql_operation(fields.statement()),
            fields.elapsed_secs,
        );
    }
}

fn emit_db_span(operation: &str, elapsed_secs: Option<f64>) {
    let end_time = SystemTime::now();
    let start_time = elapsed_secs
        .filter(|elapsed| elapsed.is_finite() && *elapsed >= 0.0)
        .and_then(|elapsed| Duration::try_from_secs_f64(elapsed).ok())
        .and_then(|duration| end_time.checked_sub(duration))
        .unwrap_or(end_time);
    let tracer = global::tracer("ryframe-db");
    let parent_context = OtelContext::current();
    let mut span = tracer
        .span_builder(format!("SQL {operation}"))
        .with_kind(SpanKind::Client)
        .with_start_time(start_time)
        .with_attributes([
            KeyValue::new(DB_SYSTEM_NAME, "mysql"),
            KeyValue::new(DB_OPERATION_NAME, operation.to_owned()),
        ])
        .start_with_context(&tracer, &parent_context);
    span.end_with_timestamp(end_time);
}
