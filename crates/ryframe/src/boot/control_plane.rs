use std::sync::Arc;

use ryframe_config::{AppConfig, MigrationMode};
use ryframe_db::{CallbackDatabaseMetricsObserver, ControlDatabaseCluster};
use ryframe_kernel::AppError;
use ryframe_tenant_db::TenantDatabaseRouter;

use super::{datasource, startup, tenant_data};

/// 控制面启动策略只描述 API 与 Worker 已存在的行为差异。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlPlaneProcess {
    Api,
    Worker,
}

/// 控制面准备所需的最小运行参数。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlPlaneStartup {
    process: ControlPlaneProcess,
    allows_initialization_writes: bool,
}

impl ControlPlaneStartup {
    pub const fn api(starts_background_tasks: bool) -> Self {
        Self {
            process: ControlPlaneProcess::Api,
            allows_initialization_writes: starts_background_tasks,
        }
    }

    pub const fn worker(allows_initialization_writes: bool) -> Self {
        Self {
            process: ControlPlaneProcess::Worker,
            allows_initialization_writes,
        }
    }

    const fn migration_mode(self, configured: MigrationMode) -> MigrationMode {
        startup::effective_migration_mode(self.allows_initialization_writes, configured)
    }
}

/// 已完成结构校验和租户目标准备的控制面依赖。
pub struct PreparedControlPlane {
    pub tenant_database_router: Arc<TenantDatabaseRouter>,
}

/// 在控制库连接建立后，完成 API 与 Worker 共用的只读或可写启动阶段。
pub async fn prepare(
    database: &ControlDatabaseCluster,
    config: &AppConfig,
    startup: ControlPlaneStartup,
) -> Result<PreparedControlPlane, AppError> {
    install_database_metrics(database);
    apply_migration(database, config.database.migration_mode, startup).await?;
    verify_schema(database, startup.process).await?;

    let tenant_database_router = Arc::new(tenant_data::build_router(database.clone(), config)?);
    tenant_data::verify_current_targets(
        &tenant_database_router,
        startup.allows_initialization_writes,
    )
    .await?;
    verify_fixed_tenant(database, config, startup.process).await?;

    Ok(PreparedControlPlane {
        tenant_database_router,
    })
}

async fn apply_migration(
    database: &ControlDatabaseCluster,
    configured: MigrationMode,
    startup: ControlPlaneStartup,
) -> Result<(), AppError> {
    match startup.migration_mode(configured) {
        MigrationMode::Auto => ryframe_db::migration::up(database.write())
            .await
            .map_err(|error| migration_error(startup.process, false, error))?,
        MigrationMode::Verify => ryframe_db::migration::verify(database.write())
            .await
            .map_err(|error| migration_error(startup.process, true, error))?,
        MigrationMode::Off => match startup.process {
            ControlPlaneProcess::Api => {
                tracing::warn!(
                    "database migration checks are disabled for the isolated environment"
                );
            }
            ControlPlaneProcess::Worker => tracing::warn!("隔离环境已关闭数据库迁移校验"),
        },
    }
    Ok(())
}

fn migration_error(
    process: ControlPlaneProcess,
    verification: bool,
    error: impl std::fmt::Display,
) -> AppError {
    let message = match (process, verification) {
        (ControlPlaneProcess::Api, false) => format!("database migration failed: {error}"),
        (ControlPlaneProcess::Api, true) => {
            format!("database migration verification failed: {error}")
        }
        (ControlPlaneProcess::Worker, false) => format!("数据库迁移失败: {error}"),
        (ControlPlaneProcess::Worker, true) => format!("数据库迁移校验失败: {error}"),
    };
    AppError::Database(message)
}

async fn verify_schema(
    database: &ControlDatabaseCluster,
    process: ControlPlaneProcess,
) -> Result<(), AppError> {
    match process {
        ControlPlaneProcess::Api => datasource::verify_schema(database).await,
        ControlPlaneProcess::Worker => {
            ryframe_db::migration::verify_current_schema(database.write())
                .await
                .map_err(|error| AppError::Internal(format!("数据库结构指纹校验失败: {error}")))
        }
    }
}

async fn verify_fixed_tenant(
    database: &ControlDatabaseCluster,
    config: &AppConfig,
    process: ControlPlaneProcess,
) -> Result<(), AppError> {
    let Some(tenant_id) = fixed_tenant_to_verify(config.multi_tenancy.fixed_tenant_id()) else {
        return Ok(());
    };
    ryframe_db::repositories::TenantRepository
        .ensure_available(database.write(), tenant_id)
        .await
        .map_err(|error| {
            AppError::Config(format!(
                "单租户模式要求内置 {tenant_id} 租户存在且可用: {error}"
            ))
        })?;
    match process {
        ControlPlaneProcess::Api => tracing::info!(tenant_id, "已启用单租户模式"),
        ControlPlaneProcess::Worker => tracing::info!(tenant_id, "Worker 已启用单租户模式"),
    }
    Ok(())
}

const fn fixed_tenant_to_verify(fixed_tenant_id: Option<&str>) -> Option<&str> {
    fixed_tenant_id
}

/// 在进程边界将底层数据库事件绑定到 Prometheus 指标。
fn install_database_metrics(database: &ControlDatabaseCluster) {
    database.set_metrics_observer(Arc::new(CallbackDatabaseMetricsObserver::new(
        Arc::new(|kind, name, healthy| {
            ryframe_adapters::metrics::set_database_node_health(name, kind.metric_label(), healthy);
        }),
        Arc::new(|target, reason| {
            ryframe_adapters::metrics::record_database_read_selection(
                target.metric_label(),
                reason.metric_label(),
            );
        }),
        Arc::new(ryframe_adapters::metrics::record_database_read_fallback),
    )));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_modes_keep_write_and_probe_migration_rules() {
        let cases = [
            (ControlPlaneStartup::api(true), MigrationMode::Auto),
            (ControlPlaneStartup::worker(true), MigrationMode::Auto),
            (ControlPlaneStartup::api(false), MigrationMode::Verify),
            (ControlPlaneStartup::worker(false), MigrationMode::Verify),
        ];
        for (startup, expected) in cases {
            assert_eq!(startup.migration_mode(MigrationMode::Auto), expected);
        }
        assert_eq!(
            ControlPlaneStartup::worker(false).migration_mode(MigrationMode::Off),
            MigrationMode::Off
        );
    }

    #[test]
    fn fixed_tenant_branch_only_validates_configured_tenant() {
        assert_eq!(fixed_tenant_to_verify(None), None);
        assert_eq!(fixed_tenant_to_verify(Some("system")), Some("system"));
    }
}
