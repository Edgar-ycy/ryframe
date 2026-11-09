use std::collections::BTreeSet;

use ryframe_sdk::RyFrameBusinessModule;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, Statement};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(super) async fn strings<C: ConnectionTrait>(db: &C, sql: String) -> Result<Vec<String>, DbErr> {
    db.query_all_raw(Statement::from_string(DbBackend::MySql, sql))
        .await?
        .iter()
        .map(|row| row.try_get_by_index::<String>(0))
        .collect()
}

/// 不过滤账本、视图或其他 schema 对象，不能把框架内部的 user_tables 当空库证明。
pub(super) async fn objects<C: ConnectionTrait>(db: &C) -> Result<Vec<String>, DbErr> {
    let mut objects = strings(db, "SELECT CONCAT('table:',TABLE_TYPE,':',TABLE_NAME) FROM information_schema.TABLES WHERE TABLE_SCHEMA=DATABASE() UNION ALL SELECT CONCAT('routine:',ROUTINE_TYPE,':',ROUTINE_NAME) FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=DATABASE() UNION ALL SELECT CONCAT('event:',EVENT_NAME) FROM information_schema.EVENTS WHERE EVENT_SCHEMA=DATABASE() UNION ALL SELECT CONCAT('trigger:',TRIGGER_NAME) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=DATABASE() ORDER BY 1".into()).await?;
    objects.sort();
    Ok(objects)
}

pub(super) fn tables(objects: &[String]) -> Result<BTreeSet<String>, DbErr> {
    objects
        .iter()
        .map(|name| {
            name.strip_prefix("table:BASE TABLE:")
                .map(str::to_owned)
                .ok_or_else(|| {
                    DbErr::Custom("目标存在视图、例程、事件或触发器，拒绝初始化/接管".into())
                })
        })
        .collect()
}

pub(super) fn tenant_tables(modules: &[RyFrameBusinessModule]) -> BTreeSet<String> {
    [
        "biz_tenant_fence",
        "biz_tenant_target_slot",
        "biz_data_scope_projection",
        "biz_data_scope_owner",
        "biz_data_scope_dept",
        "seaql_tenant_data_migrations",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(
        ryframe_tenant_db::migration::tenant_data_catalog()
            .tables()
            .iter()
            .map(|t| t.table.to_owned()),
    )
    .chain(
        modules
            .iter()
            .flat_map(|module| module.tenant_data_tables())
            .map(|table| table.table.to_owned()),
    )
    .chain(
        modules
            .iter()
            .flat_map(|module| module.migrations())
            .filter(|migration| migration.scope() == ryframe_sdk::BusinessMigrationScope::Tenant)
            .flat_map(|migration| migration.schema().tables)
            .map(|table| table.table.to_owned()),
    )
    .collect()
}

pub(super) async fn classify(
    db: &DatabaseTransaction,
    scope: &str,
    kind: &str,
    shared: bool,
    modules: &[RyFrameBusinessModule],
) -> Result<bool, DbErr> {
    let all = objects(db).await?;
    if all.is_empty() {
        return Ok(true);
    }
    let present = tables(&all)?;
    if present.contains("ryframe_resource_ownership") {
        let kinds = strings(
            db,
            "SELECT resource_kind FROM ryframe_resource_ownership ORDER BY resource_kind".into(),
        )
        .await?;
        for existing in &kinds {
            if !matches!(existing.as_str(), "control" | "tenant-data") {
                return Err(DbErr::Custom("存在未知资源归属，拒绝初始化".into()));
            }
            ryframe_db::resource_ownership::verify_resource_ownership(db, scope, existing).await?;
        }
        if kinds.iter().any(|value| value == kind) {
            return Ok(false);
        }
    }
    if kind != "tenant-data" || !shared || !present.is_disjoint(&tenant_tables(modules)) {
        return Err(DbErr::Custom(
            "非首次空库且缺少本资源归属；必须显式 ownership-adopt，up 不补标记".into(),
        ));
    }
    ryframe_db::resource_ownership::verify_resource_ownership(db, scope, "control").await?;
    ryframe_db::migration::verify(db).await?;
    let allowed = control_tables(modules)?;
    if !present.is_subset(&allowed) {
        return Err(DbErr::Custom(
            "shared-control 首次初始化前存在非控制目录对象".into(),
        ));
    }
    if present.contains("ryframe_business_migration") {
        let scopes = strings(
            db,
            "SELECT DISTINCT scope FROM ryframe_business_migration ORDER BY scope".into(),
        )
        .await?;
        if scopes.iter().any(|scope| scope != "control") {
            return Err(DbErr::Custom(
                "共有业务账本已有租户或未知范围记录，必须显式接管".into(),
            ));
        }
    }
    let migrations = modules
        .iter()
        .flat_map(|m| m.migrations())
        .filter(|m| m.scope() == ryframe_sdk::BusinessMigrationScope::Control)
        .map(|m| m.as_ref())
        .collect::<Vec<_>>();
    ryframe_sdk::verify_business_migrations_in_session(
        db,
        ryframe_sdk::BusinessMigrationScope::Control,
        &migrations,
    )
    .await
    .map_err(|error| DbErr::Custom(error.to_string()))?;
    Ok(true)
}

pub(super) fn control_tables(modules: &[RyFrameBusinessModule]) -> Result<BTreeSet<String>, DbErr> {
    let mut allowed = ryframe_db::migration::framework_initialization_assertions(&[])?
        .into_iter()
        .map(|(name, _)| name)
        .collect::<BTreeSet<_>>();
    allowed.extend([
        "seaql_migrations".into(),
        "ryframe_business_migration".into(),
    ]);
    allowed.extend(
        modules
            .iter()
            .flat_map(|m| m.control_tables())
            .map(|t| t.table.to_owned()),
    );
    allowed.extend(
        modules
            .iter()
            .flat_map(|module| module.migrations())
            .filter(|migration| migration.scope() == ryframe_sdk::BusinessMigrationScope::Control)
            .flat_map(|migration| migration.schema().tables)
            .map(|table| table.table.to_owned()),
    );
    Ok(allowed)
}

/// 初始化登记前及接管均核对完整对象目录；业务迁移的辅助表同样来自正式结构声明。
pub(super) async fn verified_tables<C: ConnectionTrait>(
    db: &C,
    kind: &str,
    shared: bool,
    modules: &[RyFrameBusinessModule],
) -> Result<BTreeSet<String>, DbErr> {
    if !matches!(kind, "control" | "tenant-data") {
        return Err(DbErr::Custom("归属资源范围非法".into()));
    }
    let present = tables(&objects(db).await?)?;
    let mut allowed = if shared || kind == "control" {
        control_tables(modules)?
    } else {
        BTreeSet::from([
            "ryframe_resource_ownership".into(),
            "ryframe_business_migration".into(),
        ])
    };
    if shared || kind == "tenant-data" {
        allowed.extend(tenant_tables(modules));
    }
    if !present.is_subset(&allowed) {
        let unknown = present.difference(&allowed).cloned().collect::<Vec<_>>();
        return Err(DbErr::Custom(format!(
            "目标存在未登记表 {}；未登记归属，核对完整业务目录和迁移，不自动认领",
            unknown.join(", ")
        )));
    }
    Ok(present)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub objects: Vec<String>,
    pub schemas: Vec<(String, String)>,
    pub metadata: Vec<(String, String)>,
    pub ownership: Vec<(String, String, String, String, String)>,
}

pub(super) async fn snapshot<C: ConnectionTrait>(db: &C) -> Result<Snapshot, DbErr> {
    let objects = objects(db).await?;
    let names = tables(&objects)?;
    let mut schemas = Vec::new();
    let mut metadata = Vec::new();
    for name in &names {
        schemas.push((
            name.clone(),
            digest(
                ryframe_db::schema::canonical_table_schema(db, name)
                    .await?
                    .as_bytes(),
            ),
        ));
        if name.starts_with("seaql_")
            || matches!(
                name.as_str(),
                "ryframe_business_migration"
                    | "biz_tenant_fence"
                    | "biz_tenant_target_slot"
                    | "sys_tenant_data_placement"
                    | "sys_tenant_data_migration"
            )
        {
            metadata.push((name.clone(), row_digest(db, name).await?));
        }
    }
    let rows = db.query_all_raw(Statement::from_string(DbBackend::MySql,
        "SELECT resource_kind,scope_id,marker,CAST(created_at AS CHAR),CAST(updated_at AS CHAR) FROM ryframe_resource_ownership ORDER BY resource_kind")).await?;
    let ownership = rows
        .iter()
        .map(|row| {
            Ok((
                row.try_get_by_index::<String>(0)?,
                row.try_get_by_index::<String>(1)?,
                digest(row.try_get_by_index::<String>(2)?.as_bytes()),
                row.try_get_by_index::<String>(3)?,
                row.try_get_by_index::<String>(4)?,
            ))
        })
        .collect::<Result<Vec<_>, DbErr>>()?;
    Ok(Snapshot {
        objects,
        schemas,
        metadata,
        ownership,
    })
}

async fn row_digest<C: ConnectionTrait>(db: &C, table: &str) -> Result<String, DbErr> {
    let columns = db.query_all_raw(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT COLUMN_NAME FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=? ORDER BY ORDINAL_POSITION", [table.into()])).await?;
    let columns = columns
        .iter()
        .map(|row| row.try_get_by_index::<String>(0))
        .collect::<Result<Vec<_>, _>>()?;
    let expressions = columns
        .iter()
        .map(|name| format!("CAST(`{}` AS CHAR)", name.replace('`', "``")))
        .collect::<Vec<_>>()
        .join(",");
    let rows = strings(
        db,
        format!(
            "SELECT CAST(JSON_ARRAY({expressions}) AS CHAR) AS value FROM `{}` ORDER BY CAST(value AS BINARY)",
            table.replace('`', "``")
        ),
    )
    .await?;
    Ok(digest(
        serde_json::to_vec(&rows)
            .map_err(|e| DbErr::Custom(e.to_string()))?
            .as_slice(),
    ))
}
