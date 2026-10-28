use ryframe_sdk::{BusinessMigrationScope, RyFrameBusinessModule};
use sea_orm::{DatabaseConnection, DatabaseTransaction, DbErr};

#[path = "ownership/adopt.rs"]
mod adopt;
#[path = "ownership/contract.rs"]
mod contract;
#[path = "ownership/fence.rs"]
mod fence;
#[path = "ownership/guard.rs"]
mod guard;
#[path = "ownership/state.rs"]
mod state;
pub(super) use adopt::{Options, parse, run};

pub(super) async fn control_up(
    db: &DatabaseConnection,
    scope: &str,
    hashes: Option<&ryframe_db::migration::BootstrapHashes>,
    modules: &[RyFrameBusinessModule],
) -> Result<(), DbErr> {
    let mut guard = guard::Guard::acquire(db).await?;
    let identity = guard.identity.clone();
    let result = async {
        let fresh = state::classify(guard.session(), scope, "control", false, modules).await?;
        ryframe_db::migration::up_in_session(guard.session_mut(), hashes).await?;
        business_up(
            guard.session_mut(),
            BusinessMigrationScope::Control,
            modules,
        )
        .await?;
        ryframe_db::migration::verify(guard.session()).await?;
        complete(
            guard.session_mut(),
            scope,
            "control",
            fresh,
            &identity,
            modules,
            false,
        )
        .await?;
        println!("scope=control migration=completed schema=verified ownership=verified");
        Ok(())
    }
    .await;
    finish(result, guard.close().await)
}

pub(super) async fn tenant_up(
    db: &DatabaseConnection,
    scope: &str,
    target: &str,
    mysql: bool,
    modules: &[RyFrameBusinessModule],
    aliases: &[String],
) -> Result<(), DbErr> {
    let mut guard = guard::Guard::acquire(db).await?;
    let identity = guard.identity.clone();
    let result = async {
        let chosen = super::targets::verify_aliases(guard.session(), aliases).await.map_err(|error| DbErr::Custom(error.to_string()))?;
        if chosen.as_deref() != Some(target) { return Err(DbErr::Custom("取得迁移互斥后别名唯一执行键变化；尚未开始迁移".into())); }
        let fresh = state::classify(guard.session(), scope, "tenant-data", !mysql, modules).await?;
        if mysql {
            ryframe_tenant_db::migration::ensure_mysql_target_boundary(guard.session()).await?;
        }
        ryframe_tenant_db::migration::up_in_session(guard.session_mut()).await?;
        business_up(guard.session_mut(), BusinessMigrationScope::Tenant, modules).await?;
        if mysql {
            ryframe_tenant_db::migration::verify_mysql_target(guard.session()).await?;
        } else {
            ryframe_tenant_db::migration::verify_shared_control(guard.session()).await?;
        }
        complete(guard.session_mut(), scope, "tenant-data", fresh, &identity, modules, !mysql).await?;
        println!("scope=tenant-data target={target} migration=completed schema=verified ownership=verified");
        Ok(())
    }
    .await;
    finish(result, guard.close().await)
}

fn finish(result: Result<(), DbErr>, closed: Result<(), DbErr>) -> Result<(), DbErr> {
    match (result, closed) {
        (Err(error), Err(close)) => Err(DbErr::Custom(format!(
            "{error}；迁移会话清理也失败：{close}；保留现场，不自动重放"
        ))),
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

async fn complete(
    db: &mut DatabaseTransaction,
    scope: &str,
    kind: &str,
    fresh: bool,
    identity: &ryframe_db::connection::DatabaseIdentity,
    modules: &[RyFrameBusinessModule],
    shared: bool,
) -> Result<(), DbErr> {
    if ryframe_db::connection::physical_identity(db).await? != *identity {
        return Err(DbErr::Custom(
            "初始化后物理身份变化；未登记归属，保留现场核对".into(),
        ));
    }
    if fresh {
        state::verified_tables(db, kind, shared, modules).await?;
        ryframe_db::resource_ownership::register_in_session(db, scope, kind).await?;
    }
    ryframe_db::resource_ownership::verify_resource_ownership(db, scope, kind).await?;
    Ok(())
}

async fn business_up(
    db: &mut DatabaseTransaction,
    scope: BusinessMigrationScope,
    modules: &[RyFrameBusinessModule],
) -> Result<(), DbErr> {
    let migrations = modules
        .iter()
        .flat_map(|m| m.migrations())
        .filter(|m| m.scope() == scope)
        .map(|m| m.as_ref())
        .collect::<Vec<_>>();
    ryframe_sdk::validate_business_migration_ledger(db, scope, &migrations)
        .await
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    for migration in &migrations {
        migration
            .up_in_session(db)
            .await
            .map_err(|e| DbErr::Custom(e.to_string()))?;
    }
    ryframe_sdk::verify_business_migrations_in_session(db, scope, &migrations)
        .await
        .map_err(|e| DbErr::Custom(e.to_string()))
}
