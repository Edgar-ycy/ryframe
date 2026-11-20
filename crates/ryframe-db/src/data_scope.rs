use ryframe_kernel::{DataScope, DataScopeContextRef};
use sea_orm::sea_query::{Alias, Expr, Query};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbBackend, ExprTrait, FromQueryResult, Statement,
};

use crate::entities::user;

/// 为带“创建人/所属用户 ID”字段的业务表生成统一数据范围条件。
/// 部门范围通过租户内 sys_user.dept_id 子查询转换为用户 ID 集合。
pub fn owner_id_condition<C>(
    owner_column: C,
    tenant_id: &str,
    ctx: &DataScopeContextRef<'_>,
) -> Option<Condition>
where
    C: ColumnTrait,
{
    match ctx.scope {
        DataScope::All => None,
        DataScope::SelfOnly => Some(Condition::all().add(owner_column.eq(ctx.user_id))),
        DataScope::Dept => Some(match ctx.dept_id {
            Some(dept_id) => owner_in_departments(owner_column, tenant_id, &[dept_id], false, ctx),
            None => Condition::all().add(Expr::cust("1 = 0")),
        }),
        DataScope::DeptAndChildren => ctx
            .dept_id
            .map(|dept_id| {
                let dept_ids = Query::select()
                    .column(crate::entities::dept::Column::Id)
                    .from(crate::entities::dept::Entity)
                    .and_where(crate::entities::dept::Column::TenantId.eq(tenant_id))
                    .and_where(
                        crate::entities::dept::Column::DelFlag
                            .eq(crate::entities::dept::Model::DEL_FLAG_NORMAL),
                    )
                    .cond_where(
                        Condition::any()
                            .add(crate::entities::dept::Column::Id.eq(dept_id))
                            .add(crate::entities::dept::Column::Ancestors.eq(dept_id.to_string()))
                            .add(
                                crate::entities::dept::Column::Ancestors
                                    .like(format!("{},%", dept_id)),
                            )
                            .add(
                                crate::entities::dept::Column::Ancestors
                                    .like(format!("%,{},%", dept_id)),
                            )
                            .add(
                                crate::entities::dept::Column::Ancestors
                                    .like(format!("%,{}", dept_id)),
                            ),
                    )
                    .take();
                let user_ids = base_user_id_query(tenant_id)
                    .and_where(user::Column::DeptId.in_subquery(dept_ids))
                    .take();
                Condition::all().add(owner_column.in_subquery(user_ids))
            })
            .or_else(|| Some(Condition::all().add(Expr::cust("1 = 0")))),
        DataScope::Custom => Some(owner_in_departments(
            owner_column,
            tenant_id,
            ctx.custom_dept_ids,
            ctx.include_self,
            ctx,
        )),
    }
}

/// 为以用户名记录操作人的日志表生成数据范围条件。
pub fn owner_username_condition<C>(
    owner_column: C,
    tenant_id: &str,
    ctx: &DataScopeContextRef<'_>,
) -> Option<Condition>
where
    C: ColumnTrait,
{
    if ctx.scope == DataScope::All {
        return None;
    }
    let mut users = base_user_name_query(tenant_id);
    match ctx.scope {
        DataScope::All => unreachable!(),
        DataScope::SelfOnly => {
            users.and_where(user::Column::Id.eq(ctx.user_id));
        }
        DataScope::Dept => {
            match ctx.dept_id {
                Some(id) => users.and_where(user::Column::DeptId.eq(id)),
                None => return Some(Condition::all().add(Expr::cust("1 = 0"))),
            };
        }
        DataScope::DeptAndChildren => match ctx.dept_id {
            Some(id) => {
                let ids = Query::select()
                    .column(crate::entities::dept::Column::Id)
                    .from(crate::entities::dept::Entity)
                    .and_where(crate::entities::dept::Column::TenantId.eq(tenant_id))
                    .cond_where(
                        Condition::any()
                            .add(crate::entities::dept::Column::Id.eq(id))
                            .add(crate::entities::dept::Column::Ancestors.eq(id.to_string()))
                            .add(crate::entities::dept::Column::Ancestors.like(format!("{},%", id)))
                            .add(
                                crate::entities::dept::Column::Ancestors
                                    .like(format!("%,{},%", id)),
                            )
                            .add(
                                crate::entities::dept::Column::Ancestors.like(format!("%,{}", id)),
                            ),
                    )
                    .take();
                users.and_where(user::Column::DeptId.in_subquery(ids));
            }
            None => return Some(Condition::all().add(Expr::cust("1 = 0"))),
        },
        DataScope::Custom => {
            let mut visible = Condition::any();
            if !ctx.custom_dept_ids.is_empty() {
                visible =
                    visible.add(user::Column::DeptId.is_in(ctx.custom_dept_ids.iter().copied()));
            }
            if ctx.include_self {
                visible = visible.add(user::Column::Id.eq(ctx.user_id));
            }
            if visible.is_empty() {
                return Some(Condition::all().add(Expr::cust("1 = 0")));
            }
            users.cond_where(visible);
        }
    }
    Some(Condition::all().add(owner_column.in_subquery(users.take())))
}

/// 为租户业务表生成只读取当前已发布投影 build 的数据范围条件。
///
/// 调用方必须先从目标库元数据取得与认证主体完全一致的 build token；
/// 本函数不再访问控制库用户或部门表。
pub fn projected_owner_id_condition<C>(
    owner_column: C,
    tenant_id: &str,
    build_token: &str,
    ctx: &DataScopeContextRef<'_>,
) -> Condition
where
    C: ColumnTrait,
{
    match ctx.scope {
        DataScope::All => Condition::all(),
        DataScope::SelfOnly => owner_column.eq(ctx.user_id).into(),
        DataScope::Dept => match ctx.dept_id {
            Some(dept_id) => owner_column
                .in_subquery(projected_users(
                    tenant_id,
                    build_token,
                    Expr::cust_with_values("o.dept_id = ?", [dept_id]),
                ))
                .into(),
            None => denied(),
        },
        DataScope::DeptAndChildren => match ctx.dept_id {
            Some(dept_id) => {
                let dept_ids = projected_departments(tenant_id, build_token, dept_id);
                owner_column
                    .in_subquery(projected_users_with_dept_scope(
                        tenant_id,
                        build_token,
                        dept_ids,
                    ))
                    .into()
            }
            None => denied(),
        },
        DataScope::Custom => {
            let mut visible = Condition::any();
            if !ctx.custom_dept_ids.is_empty() {
                visible = visible.add(owner_column.in_subquery(projected_users(
                    tenant_id,
                    build_token,
                    Expr::cust("o.dept_id").is_in(ctx.custom_dept_ids.iter().copied()),
                )));
            }
            if ctx.include_self {
                visible = visible.add(owner_column.eq(ctx.user_id));
            }
            if visible.is_empty() {
                denied()
            } else {
                visible
            }
        }
    }
}

fn projected_users(
    tenant_id: &str,
    build_token: &str,
    predicate: Expr,
) -> sea_orm::sea_query::SelectStatement {
    projected_users_with_predicate(tenant_id, build_token, [predicate])
}

fn projected_users_with_dept_scope(
    tenant_id: &str,
    build_token: &str,
    dept_ids: sea_orm::sea_query::SelectStatement,
) -> sea_orm::sea_query::SelectStatement {
    projected_users_with_predicate(
        tenant_id,
        build_token,
        [Expr::cust("o.dept_id").in_subquery(dept_ids)],
    )
}

fn projected_users_with_predicate(
    tenant_id: &str,
    build_token: &str,
    predicates: impl IntoIterator<Item = Expr>,
) -> sea_orm::sea_query::SelectStatement {
    let mut query = Query::select();
    query
        .expr(Expr::cust("o.user_id"))
        .from_as(Alias::new("biz_data_scope_owner"), Alias::new("o"))
        .and_where(Expr::cust_with_values("o.tenant_id = ?", [tenant_id]))
        .and_where(Expr::cust_with_values("o.build_token = ?", [build_token]));
    for predicate in predicates {
        query.and_where(predicate);
    }
    query.take()
}

fn projected_departments(
    tenant_id: &str,
    build_token: &str,
    dept_id: i64,
) -> sea_orm::sea_query::SelectStatement {
    let mut query = Query::select();
    query
        .expr(Expr::cust("d.dept_id"))
        .from_as(Alias::new("biz_data_scope_dept"), Alias::new("d"))
        .and_where(Expr::cust_with_values("d.tenant_id = ?", [tenant_id]))
        .and_where(Expr::cust_with_values("d.build_token = ?", [build_token]))
        .cond_where(
            Condition::any()
                .add(Expr::cust_with_values("d.dept_id = ?", [dept_id]))
                .add(Expr::cust_with_values(
                    "d.ancestors = ?",
                    [dept_id.to_string()],
                ))
                .add(Expr::cust_with_values(
                    "d.ancestors LIKE ?",
                    [format!("{dept_id},%")],
                ))
                .add(Expr::cust_with_values(
                    "d.ancestors LIKE ?",
                    [format!("%,{dept_id},%")],
                ))
                .add(Expr::cust_with_values(
                    "d.ancestors LIKE ?",
                    [format!("%,{dept_id}")],
                )),
        );
    query.take()
}

fn denied() -> Condition {
    Condition::all().add(Expr::cust("1 = 0"))
}

/// 在租户目标写事务内锁定并核对当前投影版本，返回查询使用的 build token。
pub async fn lock_projected_build<C>(
    db: &C,
    tenant_id: &str,
    expected_version: i64,
) -> ryframe_kernel::AppResult<String>
where
    C: ConnectionTrait,
{
    let row = ProjectedBuildRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT `source_version`, `build_token`, `ready` \
         FROM `biz_data_scope_projection` WHERE `tenant_id` = ? LIMIT 1 FOR UPDATE",
        [tenant_id.to_owned().into()],
    ))
    .one(db)
    .await
    .map_err(|error| {
        ryframe_kernel::AppError::Database(format!("数据范围投影元数据锁定失败: {error}"))
    })?;
    match row {
        Some(row)
            if row.ready
                && row.source_version == expected_version
                && row.build_token.len() == 64 =>
        {
            Ok(row.build_token)
        }
        Some(row) => {
            tracing::warn!(
                tenant_data_error = "data_scope_projection_stale",
                tenant_id,
                expected = expected_version,
                actual = row.source_version,
                ready = row.ready,
                build_token_valid = row.build_token.len() == 64,
                "事务数据范围投影尚未就绪；等待同步，禁止忽略版本继续访问"
            );
            Err(ryframe_kernel::AppError::TenantDataTargetUnavailable(
                format!(
                    "租户 {tenant_id} 的数据范围投影暂不可用: expected={expected_version}, actual={}；请等待投影同步后重试",
                    row.source_version
                ),
                5,
            ))
        }
        None => {
            tracing::warn!(
                tenant_data_error = "data_scope_projection_missing",
                tenant_id,
                expected = expected_version,
                "事务数据范围投影不存在；等待同步，禁止忽略版本继续访问"
            );
            Err(ryframe_kernel::AppError::TenantDataTargetUnavailable(
                format!("租户 {tenant_id} 尚无可用的数据范围投影，请等待投影同步后重试"),
                5,
            ))
        }
    }
}

#[derive(Debug, FromQueryResult)]
struct ProjectedBuildRow {
    source_version: i64,
    build_token: String,
    ready: bool,
}

fn base_user_id_query(tenant_id: &str) -> sea_orm::sea_query::SelectStatement {
    Query::select()
        .column(user::Column::Id)
        .from(user::Entity)
        .and_where(user::Column::TenantId.eq(tenant_id))
        .and_where(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
        .to_owned()
}

fn base_user_name_query(tenant_id: &str) -> sea_orm::sea_query::SelectStatement {
    Query::select()
        .column(user::Column::Username)
        .from(user::Entity)
        .and_where(user::Column::TenantId.eq(tenant_id))
        .and_where(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
        .to_owned()
}

fn owner_in_departments<C>(
    owner_column: C,
    tenant_id: &str,
    dept_ids: &[i64],
    include_self: bool,
    ctx: &DataScopeContextRef<'_>,
) -> Condition
where
    C: ColumnTrait,
{
    let mut condition = Condition::any();
    if !dept_ids.is_empty() {
        let user_ids = base_user_id_query(tenant_id)
            .and_where(user::Column::DeptId.is_in(dept_ids.iter().copied()))
            .take();
        condition = condition.add(owner_column.in_subquery(user_ids));
    }
    if include_self {
        condition = condition.add(owner_column.eq(ctx.user_id));
    }
    if condition.is_empty() {
        Condition::all().add(Expr::cust("1 = 0"))
    } else {
        condition
    }
}
