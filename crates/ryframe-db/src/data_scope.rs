use ryframe_common::{DataScope, DataScopeContext};
use sea_orm::sea_query::{Expr, Query};
use sea_orm::{ColumnTrait, Condition};

use crate::entities::user;

/// 为带“创建人/所属用户 ID”字段的业务表生成统一数据范围条件。
/// 部门范围通过租户内 sys_user.dept_id 子查询转换为用户 ID 集合。
pub fn owner_id_condition<C>(
    owner_column: C,
    tenant_id: &str,
    ctx: &DataScopeContext,
) -> Option<Condition>
where
    C: ColumnTrait,
{
    match ctx.scope {
        DataScope::All => None,
        DataScope::SelfOnly => Some(Condition::all().add(owner_column.eq(ctx.user_id))),
        DataScope::Dept => Some(match ctx.dept_id {
            Some(dept_id) => {
                owner_in_departments(owner_column, tenant_id, vec![dept_id], false, ctx)
            }
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
            ctx.custom_dept_ids.clone(),
            ctx.include_self,
            ctx,
        )),
    }
}

/// 为以用户名记录操作人的日志表生成数据范围条件。
pub fn owner_username_condition<C>(
    owner_column: C,
    tenant_id: &str,
    ctx: &DataScopeContext,
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
                visible = visible.add(user::Column::DeptId.is_in(ctx.custom_dept_ids.clone()));
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
    dept_ids: Vec<i64>,
    include_self: bool,
    ctx: &DataScopeContext,
) -> Condition
where
    C: ColumnTrait,
{
    let mut condition = Condition::any();
    if !dept_ids.is_empty() {
        let user_ids = base_user_id_query(tenant_id)
            .and_where(user::Column::DeptId.is_in(dept_ids))
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
