use std::{sync::Arc, time::Duration as StdDuration};

use chrono::{DateTime, Utc};
use ryframe_db::{EnqueueBackgroundJob, EnqueueBackgroundJobResult};
use ryframe_kernel::AppResult;
use tokio::{sync::watch, task::JoinHandle, time};

use super::JobQueue;
use crate::system::{EXPORT_CLEANUP_JOB_TYPE, MESSAGE_RETENTION_JOB_TYPE};

impl JobQueue {
    /// 旧版按 UTC 自然日幂等写入消息过期清理任务。
    #[deprecated(note = "请使用数据库 Cron 计划；该兼容入口将在移除 Cron 功能时一并删除")]
    pub async fn enqueue_message_retention(&self) -> AppResult<EnqueueBackgroundJobResult> {
        self.enqueue_legacy_daily_cleanup(MESSAGE_RETENTION_JOB_TYPE, "message:retention")
            .await
    }

    /// 旧版按 UTC 自然日幂等写入导出结果清理任务。
    #[deprecated(note = "请使用数据库 Cron 计划；该兼容入口将在移除 Cron 功能时一并删除")]
    pub async fn enqueue_export_cleanup(&self) -> AppResult<EnqueueBackgroundJobResult> {
        self.enqueue_legacy_daily_cleanup(EXPORT_CLEANUP_JOB_TYPE, "export:cleanup")
            .await
    }

    async fn enqueue_legacy_daily_cleanup(
        &self,
        job_type: &str,
        dedupe_prefix: &str,
    ) -> AppResult<EnqueueBackgroundJobResult> {
        let now = self.database_now().await?;
        let day = now.format("%F").to_string();
        let trace_context = crate::trace_context::current_trace_context();
        let result = self
            .repository()
            .enqueue(
                self.primary(),
                EnqueueBackgroundJob {
                    tenant_id: None,
                    schedule_id: None,
                    scheduled_for: Some(now),
                    max_runtime_seconds: None,
                    job_type: job_type.to_owned(),
                    payload: serde_json::json!({ "run_date": day }),
                    priority: -10,
                    available_at: now,
                    max_attempts: 20,
                    dedupe_key: Some(format!("{dedupe_prefix}:{day}")),
                    traceparent: trace_context.traceparent,
                    tracestate: trace_context.tracestate,
                },
                now,
            )
            .await?;
        self.notify_background_jobs().await;
        Ok(result)
    }
}

/// 旧版每日清理调度器，仅为兼容下游调用保留。
///
/// 新代码必须使用数据库 Cron 计划，不能同时启动该调度器。
#[deprecated(note = "请使用 JobScheduleService；该兼容调度器将在移除 Cron 功能时一并删除")]
pub fn spawn_message_retention_scheduler(
    queue: Arc<JobQueue>,
    messaging_enabled: bool,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if messaging_enabled
                && let Err(error) = queue
                    .enqueue_legacy_daily_cleanup(MESSAGE_RETENTION_JOB_TYPE, "message:retention")
                    .await
            {
                tracing::warn!(%error, "无法写入每日消息过期清理任务");
            }
            if let Err(error) = queue
                .enqueue_legacy_daily_cleanup(EXPORT_CLEANUP_JOB_TYPE, "export:cleanup")
                .await
            {
                tracing::warn!(%error, "无法写入每日导出结果清理任务");
            }
            let now = queue.database_now().await.unwrap_or_else(|error| {
                tracing::warn!(%error, "无法读取数据库时间，按本机 UTC 时间安排下次清理");
                Utc::now()
            });
            let delay = duration_until_next_utc_day(now);
            tokio::select! {
                _ = time::sleep(delay) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    })
}

fn duration_until_next_utc_day(now: DateTime<Utc>) -> StdDuration {
    let Some(tomorrow) = now.date_naive().succ_opt() else {
        return StdDuration::from_secs(24 * 60 * 60);
    };
    let Some(next) = tomorrow.and_hms_opt(0, 0, 5) else {
        return StdDuration::from_secs(24 * 60 * 60);
    };
    (next.and_utc() - now)
        .to_std()
        .unwrap_or_else(|_| StdDuration::from_secs(60))
}
