use super::super::state;
use super::{Manifest, Plan};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement};
use std::error::Error;

pub(super) async fn verify(
    db: &DatabaseTransaction,
    kind: &str,
    modules: &[ryframe_sdk::RyFrameBusinessModule],
    shared: bool,
) -> Result<(), Box<dyn Error>> {
    let present = state::verified_tables(db, kind, shared, modules).await?;
    let scope = if kind == "control" {
        ryframe_sdk::BusinessMigrationScope::Control
    } else {
        ryframe_sdk::BusinessMigrationScope::Tenant
    };
    if shared {
        ryframe_db::migration::verify(db).await?;
        if !present.is_disjoint(&state::tenant_tables(modules)) {
            ryframe_tenant_db::migration::verify_shared_control(db).await?;
            let migrations = modules
                .iter()
                .flat_map(|m| m.migrations())
                .filter(|m| m.scope() == ryframe_sdk::BusinessMigrationScope::Tenant)
                .map(|m| m.as_ref())
                .collect::<Vec<_>>();
            ryframe_sdk::verify_business_migrations_in_session(
                db,
                ryframe_sdk::BusinessMigrationScope::Tenant,
                &migrations,
            )
            .await?;
        }
    }
    if kind == "tenant-data" {
        if shared {
            ryframe_tenant_db::migration::verify_shared_control(db).await?;
        } else {
            ryframe_tenant_db::migration::verify_mysql_target(db).await?;
        }
    }
    let migrations = modules
        .iter()
        .flat_map(|m| m.migrations())
        .filter(|m| m.scope() == scope)
        .map(|m| m.as_ref())
        .collect::<Vec<_>>();
    ryframe_sdk::verify_business_migrations_in_session(db, scope, &migrations).await?;
    Ok(())
}

pub(super) fn validate_before(manifest: &Manifest) -> Result<(), Box<dyn Error>> {
    ryframe_db::resource_ownership::validate_marker_input(
        &manifest.scope_id,
        &manifest.resource_kind,
    )?;
    for (kind, scope, hash, _, _) in &manifest.before.ownership {
        if !matches!(kind.as_str(), "control" | "tenant-data")
            || scope != &manifest.scope_id
            || hash
                != &state::digest(ryframe_db::resource_ownership::marker(scope, kind).as_bytes())
        {
            return Err("已有归属矛盾，拒绝接管；不会覆盖".into());
        }
        if kind == &manifest.resource_kind {
            return Err("本资源已有归属；核对原计划或正常 up，不重复生成接管计划".into());
        }
    }
    Ok(())
}

pub(super) fn inspect(plan: &Plan, current: &Manifest) -> Result<bool, Box<dyn Error>> {
    if plan.manifest.version != 1
        || plan.fingerprint != state::digest(&serde_json::to_vec(&plan.manifest)?)
    {
        return Err("原归属计划版本或摘要错误".into());
    }
    validate_before(&plan.manifest)?;
    if current == &plan.manifest {
        return Ok(false);
    }
    let mut without_new_row = current.clone();
    let expected_hash = state::digest(
        ryframe_db::resource_ownership::marker(
            &plan.manifest.scope_id,
            &plan.manifest.resource_kind,
        )
        .as_bytes(),
    );
    let new_rows = without_new_row
        .before
        .ownership
        .iter()
        .filter(|(kind, scope, hash, _, _)| {
            kind == &plan.manifest.resource_kind
                && scope == &plan.manifest.scope_id
                && hash == &expected_hash
        })
        .count();
    without_new_row
        .before
        .ownership
        .retain(|(kind, _, _, _, _)| kind != &plan.manifest.resource_kind);
    if new_rows == 1 && without_new_row == plan.manifest {
        return Ok(true);
    }
    Err(
        "归属计划的物理身份、配置、来源、完整结构/账本/fence或归属前后像不一致；未写入，不自动重放"
            .into(),
    )
}

pub(super) async fn apply<P: ConnectionTrait>(
    db: &mut DatabaseTransaction,
    plan: &Plan,
    expected: &ryframe_db::connection::DatabaseIdentity,
    inputs: (
        Option<&P>,
        &std::path::Path,
        ryframe_config::Environment,
        &[(String, String)],
    ),
    config: &ryframe_config::AppConfig,
    target: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    if db.is_open() {
        db.finish_mysql_root(false).await?;
    }
    db.begin_mysql_root(
        Some(sea_orm::IsolationLevel::Serializable),
        Some(sea_orm::AccessMode::ReadWrite),
    )
    .await?;
    let tx = db;
    let result = async {
        tx.query_all_raw(Statement::from_string(DbBackend::MySql,
            "SELECT resource_kind FROM ryframe_resource_ownership ORDER BY resource_kind FOR UPDATE")).await?;
        for object in &plan.manifest.before.objects {
            let name = object.strip_prefix("table:BASE TABLE:").ok_or("计划包含非表对象")?;
            tx.query_all_raw(Statement::from_string(DbBackend::MySql,
                format!("SELECT 1 FROM `{}` LIMIT 0",name.replace('`',"``")))).await?;
        }
        if ryframe_db::connection::physical_identity(tx).await? != *expected {
            return Err("写入事务物理身份变化".into());
        }
        if state::snapshot(tx).await? != plan.manifest.before { return Err("写入锁内完整前像漂移".into()); }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::MySql,
            "INSERT INTO ryframe_resource_ownership(resource_kind,scope_id,marker) VALUES (?,?,?)",
            [plan.manifest.resource_kind.clone().into(),plan.manifest.scope_id.clone().into(),
                ryframe_db::resource_ownership::marker(&plan.manifest.scope_id,&plan.manifest.resource_kind).into()])).await?;
        let mut expected_after = plan.manifest.clone();
        expected_after.before = state::snapshot(tx).await?;
        if !inspect(plan,&expected_after)? { return Err("归属写入后像不匹配".into()); }
        if super::configuration(inputs.1,inputs.2)? != inputs.3 {
            return Err("归属提交前配置漂移".into());
        }
        let primary_identity = match inputs.0 {
            Some(primary) => ryframe_db::connection::physical_identity(primary).await?,
            None => ryframe_db::connection::physical_identity(tx).await?,
        };
        if (primary_identity.server_uuid.clone(),primary_identity.database.clone()) != plan.manifest.primary_target {
            return Err("归属提交前控制库物理身份漂移".into());
        }
        if let Some(primary) = inputs.0
            && super::super::fence::verify(tx,primary,config,target).await? != plan.manifest.primary_fences {
                return Err("归属提交前控制面放置/fence 来源漂移".into());
        }
        Ok::<_,Box<dyn Error>>(())
    }.await;
    if let Err(error) = result {
        if let Err(rollback) = tx.finish_mysql_root(false).await {
            return Err(format!(
                "{error}；归属回滚也失败：{rollback}；先 inspect 原计划，不自动重放"
            )
            .into());
        }
        return Err(error);
    }
    tx.finish_mysql_root(true)
        .await
        .map_err(|error| format!("归属提交结果未知：{error}；先 inspect 原计划，不自动重放").into())
}
