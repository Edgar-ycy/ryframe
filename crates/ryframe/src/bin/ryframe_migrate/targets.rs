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
    unique_targets(registry, &selected).await?;
    Ok(selected)
}

/// 只有共享 MySQL 数据面且没有 dedicated 槽语义的别名才能合并执行。
pub(super) async fn unique_targets(
    registry: &TenantDatabaseTargetRegistry,
    selected: &BTreeMap<String, DatabaseIdentity>,
) -> Result<Vec<String>, Box<dyn Error>> {
    use ryframe_config::TenantDatabaseTargetMode;
    let mut groups: BTreeMap<&DatabaseIdentity, Vec<&String>> = BTreeMap::new();
    for (key, identity) in selected {
        groups.entry(identity).or_default().push(key);
    }
    let mut targets = Vec::new();
    for (identity, keys) in groups {
        if keys.len() > 1 {
            if keys.iter().any(|key| {
                registry.target_kind(key) != Some(TenantDatabaseTargetKind::Mysql)
                    || registry.target_mode(key) != Some(TenantDatabaseTargetMode::Shared)
            }) {
                reject_duplicates(selected)?;
            }
            let lease = registry.acquire(keys[0]).await?;
            verify_identity(lease.connection(), keys[0], identity).await?;
            let chosen = verify_aliases(
                lease.connection(),
                &keys.iter().map(|key| (*key).clone()).collect::<Vec<_>>(),
            )
            .await?
            .ok_or("别名组没有明确执行键")?;
            targets.push(chosen);
        } else {
            targets.push(keys[0].clone());
        }
    }
    Ok(targets)
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

async fn query_strings<C: sea_orm::ConnectionTrait>(
    db: &C,
    sql: String,
) -> Result<Vec<String>, sea_orm::DbErr> {
    db.query_all_raw(sea_orm::Statement::from_string(
        sea_orm::DbBackend::MySql,
        sql,
    ))
    .await?
    .iter()
    .map(|row| row.try_get_by_index::<String>(0))
    .collect()
}

pub(super) async fn verify_aliases<C: sea_orm::ConnectionTrait>(
    db: &C,
    keys: &[String],
) -> Result<Option<String>, Box<dyn Error>> {
    if keys.len() < 2 {
        return Ok(keys.first().cloned());
    }
    let objects = query_strings(
        db,
        "SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA=DATABASE()".into(),
    )
    .await?;
    if objects
        .iter()
        .any(|object| object == "biz_tenant_target_slot")
    {
        let occupied = query_strings(db,
                    "SELECT CAST(slot_id AS CHAR) FROM biz_tenant_target_slot WHERE tenant_id IS NOT NULL OR placement_generation IS NOT NULL OR switch_token IS NOT NULL".into()).await?;
        if !occupied.is_empty() {
            return Err("同物理目标别名具有 dedicated 槽绑定，写前拒绝合并".into());
        }
    }
    let routes = if objects.iter().any(|object| object == "biz_tenant_fence") {
        query_strings(
            db,
            "SELECT DISTINCT target_key FROM biz_tenant_fence ORDER BY target_key".into(),
        )
        .await?
    } else {
        Vec::new()
    };
    if routes.len() > 1 || routes.first().is_some_and(|route| !keys.contains(route)) {
        return Err("同物理目标存在多个路由或组外路由；不能任意选择执行键，尚未写入".into());
    }
    Ok(routes.into_iter().next().or_else(|| keys.first().cloned()))
}
