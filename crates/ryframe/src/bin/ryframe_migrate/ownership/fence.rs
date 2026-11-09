use super::state;
use ryframe_config::{AppConfig, TenantDatabaseTargetMode};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use std::error::Error;

/// 接管不修复 fence；活动放置或已完成迁移必须能独立解释每条现有 fence。
pub(super) async fn verify<C: ConnectionTrait, P: ConnectionTrait>(
    db: &C,
    primary: &P,
    config: &AppConfig,
    target: Option<&str>,
) -> Result<String, Box<dyn Error>> {
    ryframe_db::migration::verify(primary).await?;
    if target.is_some() {
        ryframe_db::resource_ownership::verify_resource_ownership(
            primary,
            config.scope_id.as_str(),
            "control",
        )
        .await?;
    }
    let primary_state = state::snapshot(primary).await?;
    let objects = state::objects(db).await?;
    if !objects
        .iter()
        .any(|name| name == "table:BASE TABLE:biz_tenant_fence")
    {
        if target.is_some() {
            return Err("租户接管缺少完整 fence".into());
        }
        return Ok(state::digest(&serde_json::to_vec(&(
            primary_state.objects,
            primary_state.schemas,
            primary_state.metadata,
        ))?));
    }
    let rows = db.query_all_raw(Statement::from_string(DbBackend::MySql,
        "SELECT tenant_id,target_key,placement_generation,state,switch_token FROM biz_tenant_fence ORDER BY tenant_id")).await?;
    let route = target
        .or_else(|| {
            config
                .tenant_data
                .targets
                .iter()
                .find(|t| t.kind == ryframe_config::TenantDatabaseTargetKind::Control)
                .map(|t| t.key.as_str())
        })
        .ok_or("同库租户面没有明确路由登记")?;
    verify_placements(primary, route, &rows).await?;
    for row in &rows {
        let tenant = row.try_get_by_index::<String>(0)?;
        let key = row.try_get_by_index::<String>(1)?;
        let generation = row.try_get_by_index::<i64>(2)?;
        let status = row.try_get_by_index::<String>(3)?;
        let token = row.try_get_by_index::<String>(4)?;
        if route != key || generation <= 0 {
            return Err("fence 路由身份与显式接管目标不一致".into());
        }
        let sql = match status.as_str() {
            "active" => {
                "SELECT tenant_id FROM sys_tenant_data_placement WHERE tenant_id=? AND current_target_key=? AND placement_generation=? AND switch_token=? AND state='active'"
            }
            "frozen" => {
                "SELECT tenant_id FROM sys_tenant_data_migration WHERE tenant_id=? AND source_target_key=? AND source_generation=? AND source_switch_token=? AND state IN ('succeeded','retention_pending','finalizing','finalized')"
            }
            _ => return Err("fence 状态未知".into()),
        };
        let found = primary
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                sql,
                [tenant.into(), key.into(), generation.into(), token.into()],
            ))
            .await?;
        if found.len() != 1 {
            return Err("fence 不能由唯一活动放置或已完成迁移解释；先恢复/核对迁移，不接管".into());
        }
    }
    if let Some(key) = target {
        let mode = config
            .tenant_data
            .targets
            .iter()
            .find(|t| t.key == key)
            .ok_or("目标配置缺失")?
            .mode;
        let occupied = state::strings(db, "SELECT CAST(slot_id AS CHAR) FROM biz_tenant_target_slot WHERE tenant_id IS NOT NULL OR placement_generation IS NOT NULL OR switch_token IS NOT NULL".into()).await?;
        if mode == TenantDatabaseTargetMode::Shared && !occupied.is_empty() {
            return Err("共享目标具有 dedicated 槽绑定，拒绝接管".into());
        }
        let invalid = state::strings(db, "SELECT CAST(s.slot_id AS CHAR) FROM biz_tenant_target_slot s LEFT JOIN biz_tenant_fence f ON f.tenant_id=s.tenant_id AND f.placement_generation=s.placement_generation AND f.switch_token=s.switch_token WHERE s.tenant_id IS NOT NULL AND f.tenant_id IS NULL".into()).await?;
        if !invalid.is_empty() {
            return Err("dedicated 槽与 fence 不一致".into());
        }
    }
    Ok(state::digest(&serde_json::to_vec(&(
        primary_state.objects,
        primary_state.schemas,
        primary_state.metadata,
    ))?))
}

async fn verify_placements<P: ConnectionTrait>(
    primary: &P,
    route: &str,
    rows: &[sea_orm::QueryResult],
) -> Result<(), Box<dyn Error>> {
    let placements = primary.query_all_raw(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT tenant_id,placement_generation,switch_token,state FROM sys_tenant_data_placement WHERE current_target_key=? ORDER BY tenant_id",[route.into()])).await?;
    for placement in placements {
        let tenant = placement.try_get_by_index::<String>(0)?;
        let generation = placement.try_get_by_index::<i64>(1)?;
        let token = placement.try_get_by_index::<String>(2)?;
        let status = placement.try_get_by_index::<String>(3)?;
        let matching = rows
            .iter()
            .filter(|row| {
                row.try_get_by_index::<String>(0).ok().as_deref() == Some(tenant.as_str())
                    && row.try_get_by_index::<String>(1).ok().as_deref() == Some(route)
                    && row.try_get_by_index::<i64>(2).ok() == Some(generation)
                    && row.try_get_by_index::<String>(3).ok().as_deref() == Some("active")
                    && row.try_get_by_index::<String>(4).ok().as_deref() == Some(token.as_str())
            })
            .count();
        if status != "active" || matching != 1 {
            return Err("控制面放置未活动或缺少唯一对应 fence；拒绝归属接管".into());
        }
    }

    Ok(())
}
