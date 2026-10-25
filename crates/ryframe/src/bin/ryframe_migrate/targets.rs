use std::{collections::BTreeMap, error::Error};

use super::diagnostic::{Context, Writes};

use ryframe_config::{DbConnection, SqlLogLevel, TenantDatabaseTargetKind};
use ryframe_db::connection::{DatabaseIdentity, physical_identity};
use ryframe_tenant_db::TenantDatabaseTargetRegistry;
use sea_orm::DatabaseConnection;

/// 先核对全部明确选择的目标再开始 DDL；不扫描服务器或访问命名读取来源。
pub(super) async fn preflight(
    registry: &TenantDatabaseTargetRegistry,
    primary: &DbConnection,
    sql_log_level: SqlLogLevel,
    slow_threshold_ms: u64,
    targets: &[String],
    context: &Context<'_>,
) -> Result<BTreeMap<String, DatabaseIdentity>, Box<dyn Error>> {
    let mut selected = BTreeMap::new();
    for target in targets {
        let context = Context {
            target: Some(target),
            ..*context
        };
        let identity = match registry.target_kind(target) {
            Some(TenantDatabaseTargetKind::Control) => {
                let database = ryframe_db::connection::connect_with_sql_logging(
                    primary,
                    sql_log_level,
                    slow_threshold_ms,
                )
                .await
                .map_err(|error| context.error("目标连接预检", Writes::None, &error))?;
                let identity = physical_identity(&database).await;
                let closed = database.close().await;
                match (identity, closed) {
                    (Ok(identity), Ok(())) => identity,
                    (Err(error), Ok(())) => {
                        return Err(context.error("目标物理身份预检", Writes::None, &error));
                    }
                    (Ok(_), Err(error)) => {
                        return Err(context.error("预检关闭连接", Writes::None, &error));
                    }
                    (Err(error), Err(close)) => {
                        return Err(format!(
                            "{}；关闭连接时另有错误：{}",
                            context.error("目标物理身份预检", Writes::None, &error),
                            context.error("预检关闭连接", Writes::None, &close)
                        )
                        .into());
                    }
                }
            }
            Some(TenantDatabaseTargetKind::Mysql) => {
                let lease = registry.acquire(target).await.map_err(|error| {
                    context.error("独立目标连接预检（registry 不可用）", Writes::None, &error)
                })?;
                physical_identity(lease.connection())
                    .await
                    .map_err(|error| context.error("目标物理身份预检", Writes::None, &error))?
            }
            None => {
                return Err(format!(
                    "租户数据目标 {target} 未配置；核对 tenant_data.targets[].key，尚未执行迁移"
                )
                .into());
            }
        };
        selected.insert(target.clone(), identity);
    }
    reject_duplicates(&selected)?;
    Ok(selected)
}

pub(super) fn reject_duplicates(
    selected: &BTreeMap<String, DatabaseIdentity>,
) -> Result<(), Box<dyn Error>> {
    let mut owners = BTreeMap::new();
    for (target, identity) in selected {
        if let Some(previous) = owners.insert(identity, target) {
            return Err(format!("迁移目标 {previous} 与 {target} 指向同一 MySQL 实例和数据库 {}；尚未执行所选迁移。保留一个迁移目标键并修正 tenant_data.targets；合法读取别名放在 database.sources，不需要删除读取来源", identity.database).into());
        }
    }
    Ok(())
}

pub(super) async fn verify_identity(
    database: &DatabaseConnection,
    target: &str,
    expected: &DatabaseIdentity,
) -> Result<(), Box<dyn Error>> {
    if physical_identity(database).await? != *expected {
        return Err(format!("迁移目标 {target} 的物理实例或数据库在预检后发生变化；当前目标尚未执行 DDL，前面目标可能已完成，先核对各目标 status/verify，不自动重放").into());
    }
    Ok(())
}
