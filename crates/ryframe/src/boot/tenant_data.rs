use std::{collections::BTreeMap, sync::Arc, time::Duration};

use ryframe_config::{AppConfig, TenantDatabaseTargetMode};
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::AppError;
use ryframe_tenant_db::{
    TenantDataTargetHealth, TenantDatabaseRouter, TenantDatabaseTargetHealthStatus,
};

/// 构造租户数据路由器，不主动连接任何独立目标。
///
/// 现有控制库作为隐式 `shared-control` 目标复用同一个连接集群；命名业务数据源仍只
/// 属于控制库集群，不会被注册为租户数据目标。
pub fn build_router(
    control: ControlDatabaseCluster,
    config: &AppConfig,
) -> Result<TenantDatabaseRouter, AppError> {
    let router = TenantDatabaseRouter::new(
        control,
        &config.tenant_data,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .map_err(|error| AppError::Config(error.to_string()))?;
    tracing::info!(
        registered_targets = router.targets().len(),
        default_target = config.tenant_data.default_target,
        max_open_targets = config.tenant_data.max_open_targets,
        max_total_connections = config.tenant_data.max_total_connections,
        shared_target = TenantDatabaseRouter::shared_control_target_key(),
        "租户数据路由器已初始化，独立目标保持延迟连接"
    );
    Ok(router)
}

/// 校验当前 placement 实际引用的目标。单目标故障只降级其租户，不影响控制面启动。
pub async fn verify_current_targets(router: &TenantDatabaseRouter) -> Result<(), AppError> {
    let router = Arc::new(router.clone());
    spawn_target_health_updater(router.clone());
    spawn_metrics_updater(router);
    Ok(())
}

fn spawn_target_health_updater(router: Arc<TenantDatabaseRouter>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            run_target_health_snapshot(&router).await;
        }
    });
}

async fn run_target_health_snapshot(router: &TenantDatabaseRouter) {
    let reports = match router.verify_current_targets().await {
        Ok(reports) => reports,
        Err(error) => {
            tracing::error!(%error, "租户数据 active placement 后台健康快照失败");
            return;
        }
    };
    for report in reports {
        match report.health {
            TenantDataTargetHealth::Verified => tracing::info!(
                target = %report.target_key,
                tenant_count = report.tenant_count,
                "租户数据目标 schema 已验证"
            ),
            TenantDataTargetHealth::UnknownTarget => tracing::error!(
                target = %report.target_key,
                tenant_count = report.tenant_count,
                "placement 引用了未注册目标；相关租户保持故障隔离"
            ),
            TenantDataTargetHealth::Unavailable => tracing::error!(
                target = %report.target_key,
                tenant_count = report.tenant_count,
                "租户数据目标连接或 schema 不可用；相关租户保持故障隔离"
            ),
        }
    }
    update_metrics(router).await;
}

fn spawn_metrics_updater(router: Arc<TenantDatabaseRouter>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // 首次快照已由启动校验同步写入。
        interval.tick().await;
        loop {
            interval.tick().await;
            update_metrics(&router).await;
        }
    });
}

async fn update_metrics(router: &TenantDatabaseRouter) {
    let metadata = router.targets().metadata().await;
    let mut target_counts = BTreeMap::<(&'static str, &'static str), usize>::new();
    for target in metadata {
        let mode = mode_label(Some(target.mode));
        let health = match target.health {
            TenantDatabaseTargetHealthStatus::Unknown => "unknown",
            TenantDatabaseTargetHealthStatus::Verified => "verified",
            TenantDatabaseTargetHealthStatus::Unavailable => "unavailable",
        };
        *target_counts.entry((mode, health)).or_default() += 1;
    }
    let placements = router
        .placement_metrics_snapshot()
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "租户数据低基数 placement 指标刷新失败");
            Vec::new()
        });
    let mut placement_counts = BTreeMap::<(&'static str, &'static str), u64>::new();
    for placement in placements {
        *placement_counts
            .entry((mode_label(placement.mode), placement.state.as_str()))
            .or_default() += placement.tenant_count;
    }
    ryframe_adapters::metrics::reset_tenant_data_aggregates();
    for ((mode, health), count) in target_counts {
        ryframe_adapters::metrics::set_tenant_data_target_count(mode, health, count);
    }
    for ((mode, state), count) in placement_counts {
        ryframe_adapters::metrics::set_tenant_data_placement_count(mode, state, count);
    }
    let pools = router.targets().pool_stats().await;
    ryframe_adapters::metrics::set_tenant_data_pool_stats(
        pools.open_targets,
        pools.opening_targets,
        pools.reserved_connections,
        pools.active_leases,
    );
}

const fn mode_label(mode: Option<TenantDatabaseTargetMode>) -> &'static str {
    match mode {
        Some(TenantDatabaseTargetMode::Shared) => "shared",
        Some(TenantDatabaseTargetMode::Dedicated) => "dedicated",
        None => "unknown",
    }
}
