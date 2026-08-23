use super::*;

impl JobWorker {
    /// 启动配置数量的并行消费循环，并在收到关闭信号后有序退出。
    pub fn spawn(self, shutdown: watch::Receiver<bool>) -> Vec<JoinHandle<()>> {
        let instance = Uuid::new_v4().simple().to_string();
        let mut tasks = (0..self.concurrency)
            .map(|slot| {
                let worker = self.clone();
                let worker_id = format!("{}-{slot}-{}", worker.worker_prefix, &instance[..12]);
                let receiver = shutdown.clone();
                tokio::spawn(async move {
                    worker.run_until_shutdown(worker_id, receiver).await;
                })
            })
            .collect::<Vec<_>>();

        if let Some(listener) = self.queue.spawn_wakeup_listener(shutdown.clone()) {
            tasks.push(listener);
        }

        if self.queue.has_metrics_observer() && !self.handlers.is_empty() {
            let queue = self.queue.clone();
            let execution_tenant_scope = self.execution_tenant_scope.clone();
            let job_types = self.handlers.keys().cloned().collect::<Vec<_>>();
            let mut receiver = shutdown.clone();
            tasks.push(tokio::spawn(async move {
                let mut collection_degraded = false;
                loop {
                    match queue
                        .report_metrics_for_types(&job_types, &execution_tenant_scope)
                        .await
                    {
                        Ok(()) if collection_degraded => {
                            tracing::info!("后台任务队列指标采集已恢复");
                            collection_degraded = false;
                        }
                        Ok(()) => {}
                        Err(error) if collection_degraded => {
                            tracing::debug!(%error, "后台任务队列指标采集仍不可用");
                        }
                        Err(error) => {
                            tracing::warn!(%error, "后台任务队列指标采集失败");
                            collection_degraded = true;
                        }
                    }
                    tokio::select! {
                        _ = time::sleep(StdDuration::from_secs(30)) => {}
                        changed = receiver.changed() => {
                            if changed.is_err() || *receiver.borrow() {
                                break;
                            }
                        }
                    }
                }
            }));
        }

        let worker = self.clone();
        let lease_recovery_shutdown = shutdown.clone();
        tasks.push(tokio::spawn(async move {
            worker
                .recover_expired_leases_until_shutdown(lease_recovery_shutdown)
                .await;
        }));

        let reconcilers = self
            .handlers
            .values()
            .filter(|handler| handler.has_authoritative_reconciler())
            .cloned()
            .collect::<Vec<_>>();
        if !reconcilers.is_empty() {
            let mut receiver = shutdown;
            tasks.push(tokio::spawn(async move {
                loop {
                    if *receiver.borrow() {
                        break;
                    }
                    for reconciler in &reconcilers {
                        if let Err(error) = reconciler.reconcile_authoritative_jobs().await {
                            tracing::warn!(
                                job_type = reconciler.job_type(),
                                error_code = %error.error_code(),
                                "权威业务任务 watchdog 对账失败"
                            );
                        }
                    }
                    tokio::select! {
                        _ = time::sleep(StdDuration::from_secs(30)) => {}
                        changed = receiver.changed() => {
                            if changed.is_err() || *receiver.borrow() {
                                break;
                            }
                        }
                    }
                }
            }));
        }

        tasks
    }

    async fn run_until_shutdown(&self, worker_id: String, mut shutdown: watch::Receiver<bool>) {
        tracing::info!(worker_id = %worker_id, "后台任务 Worker 已启动");
        let mut consecutive_infrastructure_failures = 0_u32;
        let mut idle_wait = self.poll_interval;
        let mut wakeups = self.queue.subscribe_background_job_wakeups();
        loop {
            if *shutdown.borrow() {
                break;
            }

            match self.run_once(&worker_id).await {
                Ok(JobRunResult::Idle) => {
                    if consecutive_infrastructure_failures > 0 {
                        tracing::info!(worker_id = %worker_id, "后台任务 Worker 基础设施调用已恢复");
                    }
                    consecutive_infrastructure_failures = 0;
                    idle_wait =
                        next_idle_wait(idle_wait, self.poll_interval, self.max_idle_poll_interval);
                    tokio::select! {
                        _ = time::sleep(jittered_delay(idle_wait)) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                        changed = wakeups.changed() => {
                            if changed.is_err() {
                                break;
                            }
                            idle_wait = self.poll_interval;
                        }
                    }
                }
                Ok(JobRunResult::LeaseLost) => {
                    if consecutive_infrastructure_failures > 0 {
                        tracing::info!(worker_id = %worker_id, "后台任务 Worker 基础设施调用已恢复");
                    }
                    consecutive_infrastructure_failures = 0;
                    idle_wait = self.poll_interval;
                    tracing::warn!(worker_id = %worker_id, "后台任务租约已失效，忽略本次处理结果");
                }
                Ok(_) => {
                    if consecutive_infrastructure_failures > 0 {
                        tracing::info!(worker_id = %worker_id, "后台任务 Worker 基础设施调用已恢复");
                    }
                    consecutive_infrastructure_failures = 0;
                    idle_wait = self.poll_interval;
                }
                Err(error) => {
                    consecutive_infrastructure_failures =
                        consecutive_infrastructure_failures.saturating_add(1);
                    let delay = infrastructure_retry_delay(
                        self.poll_interval,
                        consecutive_infrastructure_failures,
                    );
                    if consecutive_infrastructure_failures == 1 {
                        tracing::warn!(
                            worker_id = %worker_id,
                            error = %error,
                            delay_ms = delay.as_millis(),
                            "后台任务 Worker 基础设施调用失败，将退避后重试"
                        );
                    } else {
                        tracing::debug!(
                            worker_id = %worker_id,
                            error = %error,
                            consecutive_infrastructure_failures,
                            delay_ms = delay.as_millis(),
                            "后台任务 Worker 基础设施调用仍不可用"
                        );
                    }
                    tokio::select! {
                        _ = time::sleep(delay) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                    }
                }
            }
        }
        tracing::info!(worker_id = %worker_id, "后台任务 Worker 已停止");
    }

    /// 单独回收过期租约，避免与并发领取任务共享同一事务。
    async fn recover_expired_leases_until_shutdown(&self, mut shutdown: watch::Receiver<bool>) {
        let mut recovery_degraded = false;
        loop {
            if *shutdown.borrow() {
                break;
            }
            match self
                .queue
                .recover_expired_leases(&self.execution_tenant_scope)
                .await
            {
                Ok(()) if recovery_degraded => {
                    tracing::info!("后台任务过期租约回收已恢复");
                    recovery_degraded = false;
                }
                Ok(()) => {}
                Err(error) if recovery_degraded => {
                    tracing::debug!(%error, "后台任务过期租约回收仍不可用");
                }
                Err(error) => {
                    tracing::warn!(%error, "后台任务过期租约回收失败，将在下次轮询重试");
                    recovery_degraded = true;
                }
            }
            tokio::select! {
                _ = time::sleep(self.lease_recovery_interval) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    }
}
