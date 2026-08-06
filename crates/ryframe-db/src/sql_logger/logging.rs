use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc::{SyncSender, TrySendError, sync_channel},
};
use std::thread::JoinHandle;

use ryframe_config::SqlLogLevel;
use tracing::{Dispatch, Event, dispatcher::WeakDispatch, span::Id};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use super::fields::{SqlxEventFields, clean_sql, extract_sql_operation};

const NORMALIZED_SQL_QUEUE_CAPACITY: usize = 4_096;

/// 将 SQLx 查询事件转换成统一的结构化 tracing 事件。
///
/// tracing 会阻止 Layer 回调中的递归事件派发，因此规范化记录通过有界队列交给
/// 独立线程发送；最终事件仍进入当前 subscriber，共用格式、writer 和滚动策略。
pub struct SqlLogLayer {
    level: SqlLogLevel,
    slow_threshold_ms: u64,
    sender: SyncSender<SqlLogQueueItem>,
    overflow: Arc<SqlLogQueueOverflow>,
    dispatch: Mutex<Option<WeakDispatch>>,
}

/// 跨线程累计因队列饱和而未能规范化的 SQL 日志数；工作线程会在恢复消费后合并告警。
#[derive(Default)]
struct SqlLogQueueOverflow {
    dropped_records: AtomicU64,
}

/// SQL 规范化线程的关闭 Guard，确保应用退出前已排空已入队的 SQL 日志。
pub struct SqlLogGuard {
    sender: Option<SyncSender<SqlLogQueueItem>>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for SqlLogGuard {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            // FIFO 关闭标记位于所有已提交记录之后，线程会先完成这些记录再退出。
            let _ = sender.send(SqlLogQueueItem::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl SqlLogLayer {
    /// 创建兼容的独立 Layer；调用方若需要在关闭时排空队列，应改用 `with_guard`。
    pub fn new(level: SqlLogLevel, slow_threshold_ms: u64) -> Self {
        let (layer, _worker) = Self::spawn(level, slow_threshold_ms);
        layer
    }

    /// 创建 Layer 与其关闭 Guard，使规范化日志与主日志 writer 一起收尾。
    pub fn with_guard(level: SqlLogLevel, slow_threshold_ms: u64) -> (Self, SqlLogGuard) {
        let (layer, worker) = Self::spawn(level, slow_threshold_ms);
        let guard = SqlLogGuard {
            sender: Some(layer.sender.clone()),
            worker: Some(worker),
        };
        (layer, guard)
    }

    fn spawn(level: SqlLogLevel, slow_threshold_ms: u64) -> (Self, JoinHandle<()>) {
        let (sender, receiver) = sync_channel::<SqlLogQueueItem>(NORMALIZED_SQL_QUEUE_CAPACITY);
        let overflow = Arc::new(SqlLogQueueOverflow::default());
        let worker_overflow = Arc::clone(&overflow);
        let worker = std::thread::Builder::new()
            .name("ryframe-sql-log".into())
            .spawn(move || {
                while let Ok(item) = receiver.recv() {
                    match item {
                        SqlLogQueueItem::Record(record) => {
                            record.emit();
                            let dropped_records =
                                worker_overflow.dropped_records.swap(0, Ordering::AcqRel);
                            if dropped_records > 0 {
                                record.emit_queue_overflow_warning(dropped_records);
                            }
                        }
                        SqlLogQueueItem::Shutdown => break,
                    }
                }
            })
            .expect("无法启动 SQL 日志规范化线程");
        (
            Self {
                level,
                slow_threshold_ms,
                sender,
                overflow,
                dispatch: Mutex::new(None),
            },
            worker,
        )
    }
}

impl<S> Layer<S> for SqlLogLayer
where
    S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_register_dispatch(&self, subscriber: &Dispatch) {
        *self
            .dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(subscriber.downgrade());
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" || self.level == SqlLogLevel::Off {
            return;
        }

        let fields = SqlxEventFields::from_event(event);
        let elapsed_ms = fields.elapsed_ms();
        let slow = fields.slow || elapsed_ms >= self.slow_threshold_ms as f64;
        if self.level == SqlLogLevel::Slow && !slow {
            return;
        }

        let Some(dispatch) = self
            .dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(WeakDispatch::upgrade)
        else {
            return;
        };
        let record = SqlLogRecord {
            dispatch,
            parent: ctx.event_span(event).map(|span| span.id().clone()),
            summary: clean_sql(fields.summary()),
            statement: (self.level == SqlLogLevel::Slow || self.level == SqlLogLevel::Full)
                .then(|| clean_sql(fields.statement())),
            operation: extract_sql_operation(fields.statement()),
            returned_rows: fields.rows_returned.unwrap_or_default(),
            affected_rows: fields.rows_affected.unwrap_or_default(),
            elapsed_ms,
            slow,
            slow_threshold_ms: self.slow_threshold_ms,
        };

        match self.sender.try_send(SqlLogQueueItem::Record(record)) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                self.overflow
                    .dropped_records
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

enum SqlLogQueueItem {
    Record(SqlLogRecord),
    Shutdown,
}

struct SqlLogRecord {
    dispatch: Dispatch,
    parent: Option<Id>,
    summary: String,
    statement: Option<String>,
    operation: &'static str,
    returned_rows: u64,
    affected_rows: u64,
    elapsed_ms: f64,
    slow: bool,
    slow_threshold_ms: u64,
}

impl SqlLogRecord {
    fn emit(&self) {
        let dispatch = self.dispatch.clone();
        tracing::dispatcher::with_default(&dispatch, || {
            if self.slow {
                if let Some(statement) = self.statement.as_deref() {
                    tracing::event!(
                        target: "ryframe.sql",
                        parent: self.parent.clone(),
                        tracing::Level::WARN,
                        {
                            "event.kind" = "db.query",
                            db.system.name = "mysql",
                            db.operation.name = self.operation,
                            db.query.summary = %self.summary,
                            db.query.text = %statement,
                            db.response.returned_rows = self.returned_rows,
                            db.response.affected_rows = self.affected_rows,
                            duration_ms = self.elapsed_ms,
                            slow = true,
                            slow_threshold_ms = self.slow_threshold_ms,
                        },
                        "慢 SQL"
                    );
                } else {
                    tracing::event!(
                        target: "ryframe.sql",
                        parent: self.parent.clone(),
                        tracing::Level::WARN,
                        {
                            "event.kind" = "db.query",
                            db.system.name = "mysql",
                            db.operation.name = self.operation,
                            db.query.summary = %self.summary,
                            db.response.returned_rows = self.returned_rows,
                            db.response.affected_rows = self.affected_rows,
                            duration_ms = self.elapsed_ms,
                            slow = true,
                            slow_threshold_ms = self.slow_threshold_ms,
                        },
                        "慢 SQL"
                    );
                }
            } else if let Some(statement) = self.statement.as_deref() {
                tracing::event!(
                    target: "ryframe.sql",
                    parent: self.parent.clone(),
                    tracing::Level::INFO,
                    {
                        "event.kind" = "db.query",
                        db.system.name = "mysql",
                        db.operation.name = self.operation,
                        db.query.summary = %self.summary,
                        db.query.text = %statement,
                        db.response.returned_rows = self.returned_rows,
                        db.response.affected_rows = self.affected_rows,
                        duration_ms = self.elapsed_ms,
                        slow = false,
                    },
                    "SQL 查询"
                );
            } else {
                tracing::event!(
                    target: "ryframe.sql",
                    parent: self.parent.clone(),
                    tracing::Level::INFO,
                    {
                        "event.kind" = "db.query",
                        db.system.name = "mysql",
                        db.operation.name = self.operation,
                        db.query.summary = %self.summary,
                        db.response.returned_rows = self.returned_rows,
                        db.response.affected_rows = self.affected_rows,
                        duration_ms = self.elapsed_ms,
                        slow = false,
                    },
                    "SQL 查询"
                );
            }
        });
    }

    /// 队列恢复消费后输出一次聚合告警，不阻塞 SQLx 回调，也不暴露任何 SQL 参数值。
    fn emit_queue_overflow_warning(&self, dropped_records: u64) {
        let dispatch = self.dispatch.clone();
        tracing::dispatcher::with_default(&dispatch, || {
            tracing::event!(
                target: "ryframe.sql",
                parent: self.parent.clone(),
                tracing::Level::WARN,
                {
                    "event.kind" = "db.query.log_overflow",
                    dropped_records,
                },
                "SQL 日志规范化队列已饱和，部分记录被丢弃"
            );
        });
    }
}
