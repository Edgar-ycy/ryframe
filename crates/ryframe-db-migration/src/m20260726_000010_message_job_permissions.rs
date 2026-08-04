use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
use sea_orm_migration::prelude::*;

/// 为消息中心和任务监控补齐路由所需的权限记录。
///
/// 租户级权限按已有租户的权限树逐租户创建，不自动把新能力授予普通角色；
/// 平台权限只存在于 system 租户，避免普通租户取得跨租户操作能力。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table("sys_permission").await? {
            return Ok(());
        }

        seed_permissions(manager.get_connection()).await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "消息中心和任务监控权限为前向兼容数据，不能自动删除".into(),
        ))
    }
}

/// 在规范种子写入后或升级已有数据库时幂等补齐消息与任务权限。
pub(crate) async fn seed_permissions<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let tenants = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT DISTINCT tenant_id FROM sys_permission".to_owned(),
        ))
        .await?;

    for tenant in tenants {
        let tenant_id = String::try_get_by_index(&tenant, 0)?;
        for spec in PERMISSION_SPECS {
            if spec.system_only && tenant_id != SYSTEM_TENANT_ID {
                continue;
            }
            insert_permission_if_missing(db, &tenant_id, spec).await?;
        }
    }
    Ok(())
}

struct PermissionSpec {
    code: &'static str,
    name: &'static str,
    parent_code: &'static str,
    sort: i32,
    system_only: bool,
}

const SYSTEM_TENANT_ID: &str = "system";

const PERMISSION_SPECS: &[PermissionSpec] = &[
    PermissionSpec {
        code: "system:message:publish",
        name: "发布消息",
        parent_code: "system:notice",
        sort: 10,
        system_only: false,
    },
    PermissionSpec {
        code: "platform:message:publish",
        name: "跨租户发布消息",
        parent_code: "tenant:manage",
        sort: 10,
        system_only: true,
    },
    PermissionSpec {
        code: "monitor:job:list",
        name: "任务监控查询",
        parent_code: "monitor:runtime",
        sort: 10,
        system_only: false,
    },
    PermissionSpec {
        code: "monitor:job:retry",
        name: "任务人工重试",
        parent_code: "monitor:runtime",
        sort: 11,
        system_only: false,
    },
];

async fn insert_permission_if_missing<C>(
    db: &C,
    tenant_id: &str,
    spec: &PermissionSpec,
) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if permission_id(db, tenant_id, spec.code).await?.is_some() {
        return Ok(());
    }

    let parent_id = permission_id(db, tenant_id, spec.parent_code).await?;
    let statement = Query::insert()
        .into_table(Alias::new("sys_permission"))
        .columns([
            Alias::new("id"),
            Alias::new("tenant_id"),
            Alias::new("name"),
            Alias::new("code"),
            Alias::new("parent_id"),
            Alias::new("perm_type"),
            Alias::new("icon"),
            Alias::new("sort"),
            Alias::new("status"),
            Alias::new("created_at"),
            Alias::new("updated_at"),
        ])
        .values_panic([
            ryframe_utils::snowflake::try_next_snowflake_id()
                .map_err(|error| DbErr::Custom(error.to_string()))?
                .into(),
            tenant_id.into(),
            spec.name.into(),
            spec.code.into(),
            Expr::value(parent_id),
            "api".into(),
            Expr::value(Option::<String>::None),
            spec.sort.into(),
            "1".into(),
            Expr::current_timestamp(),
            Expr::current_timestamp(),
        ])
        .build(MysqlQueryBuilder);
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        statement.0,
        statement.1,
    ))
    .await?;
    Ok(())
}

async fn permission_id<C>(db: &C, tenant_id: &str, code: &str) -> Result<Option<i64>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id FROM sys_permission WHERE tenant_id = ? AND code = ? LIMIT 1",
            [tenant_id.into(), code.into()],
        ))
        .await?;
    Ok(row.map(|row| i64::try_get_by_index(&row, 0)).transpose()?)
}
