use super::*;

impl TenantDatabaseRouter {
    pub async fn verify_current_targets(
        &self,
    ) -> Result<Vec<TenantDataTargetVerification>, TenantDataError> {
        let rows = CurrentTargetRow::find_by_statement(Statement::from_string(
            DbBackend::MySql,
            CURRENT_TARGETS_QUERY,
        ))
        .all(self.inner.control.write())
        .await
        .map_err(|error| {
            tracing::warn!(%error, "当前租户数据目标集合查询失败");
            TenantDataError::InvalidConfiguration("sys_tenant_data_placement 无法强一致读取".into())
        })?;

        let concurrency = Arc::new(tokio::sync::Semaphore::new(8));
        let mut tasks = tokio::task::JoinSet::new();
        for row in rows {
            let router = self.clone();
            let concurrency = concurrency.clone();
            tasks.spawn(async move {
                let tenant_count = row.tenant_count.max(0) as u64;
                let mode = router.inner.targets.target_mode(&row.current_target_key);
                let health = if mode.is_none() {
                    TenantDataTargetHealth::UnknownTarget
                } else {
                    let _permit = concurrency.acquire_owned().await.ok();
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        router.verify_target_now(&row.current_target_key),
                    )
                    .await
                    {
                        Ok(Ok(_)) => TenantDataTargetHealth::Verified,
                        Ok(Err(error)) => {
                            tracing::warn!(
                                target = %row.current_target_key,
                                tenant_count,
                                %error,
                                "租户数据目标连接失败"
                            );
                            TenantDataTargetHealth::Unavailable
                        }
                        Err(_) => {
                            tracing::warn!(
                                target = %row.current_target_key,
                                tenant_count,
                                "租户数据目标启动校验超时"
                            );
                            TenantDataTargetHealth::Unavailable
                        }
                    }
                };
                TenantDataTargetVerification {
                    target_key: row.current_target_key,
                    tenant_count,
                    mode,
                    health,
                }
            });
        }
        let mut reports = Vec::with_capacity(tasks.len());
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(report) => reports.push(report),
                Err(error) => tracing::error!(%error, "租户数据目标启动校验任务异常退出"),
            }
        }
        reports.sort_unstable_by(|left, right| left.target_key.cmp(&right.target_key));
        Ok(reports)
    }

    /// 只读控制库的低基数 placement 聚合；不连接任何目标且不包含 tenant/target 标签。
    pub async fn placement_metrics_snapshot(
        &self,
    ) -> Result<Vec<TenantDataPlacementMetric>, TenantDataError> {
        let rows = PlacementMetricRow::find_by_statement(Statement::from_string(
            DbBackend::MySql,
            PLACEMENT_METRICS_QUERY,
        ))
        .all(self.inner.control.write())
        .await
        .map_err(|error| {
            tracing::warn!(%error, "租户数据 placement 指标聚合失败");
            TenantDataError::InvalidConfiguration("sys_tenant_data_placement 指标无法读取".into())
        })?;
        rows.into_iter()
            .map(|row| {
                let state = TenantDataState::parse(&row.state).ok_or_else(|| {
                    TenantDataError::InvalidConfiguration(
                        "sys_tenant_data_placement 包含未知状态".into(),
                    )
                })?;
                Ok(TenantDataPlacementMetric {
                    mode: self.inner.targets.target_mode(&row.current_target_key),
                    state,
                    tenant_count: row.tenant_count.max(0) as u64,
                })
            })
            .collect()
    }
}
