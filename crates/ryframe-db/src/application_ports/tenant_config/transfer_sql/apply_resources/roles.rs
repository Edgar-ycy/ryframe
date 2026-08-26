use super::*;
use crate::DbResultExt;

pub(super) async fn upsert_roles_and_relations(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &[PortableRole],
    department_ids: &BTreeMap<Vec<String>, i64>,
    changed: &BTreeSet<(String, String)>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let permission_ids = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .db()?
        .into_iter()
        .map(|item| (normalize_stable_key(&item.code), item.id))
        .collect::<BTreeMap<_, _>>();
    for item in resources {
        if !changed.contains(&("role".to_owned(), normalize_stable_key(&item.code))) {
            continue;
        }
        if permission_contains_wildcard(&item.code)
            || item
                .permission_codes
                .iter()
                .any(|code| permission_contains_wildcard(code) || is_platform_only_permission(code))
        {
            return Err(AppError::Validation(
                "超级角色或通配权限不能通过配置包迁移".into(),
            ));
        }
        let old = role::Entity::find()
            .filter(role::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .db()?
            .into_iter()
            .find(|candidate| {
                normalize_stable_key(&candidate.code) == normalize_stable_key(&item.code)
            });
        if old.as_ref().is_some_and(|value| value.is_super == 1) {
            return Err(AppError::Conflict("超级角色不能被配置包覆盖".into()));
        }
        let role_id = old.as_ref().map(|value| value.id).unwrap_or(next_id()?);
        let model = role::Model {
            id: role_id,
            tenant_id: tenant_id.to_owned(),
            name: item.name.clone(),
            code: item.code.clone(),
            is_super: 0,
            data_scope: item.data_scope.clone(),
            status: item.status.clone(),
            sort: item.sort,
            remark: item.remark.clone(),
            del_flag: role::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: old.as_ref().map(|value| value.created_at).unwrap_or(now),
            updated_at: now,
        };
        save_model(transaction, old.is_some(), role::ActiveModel::from(model)).await?;
        role_permission::Entity::delete_many()
            .filter(role_permission::Column::TenantId.eq(tenant_id))
            .filter(role_permission::Column::RoleId.eq(role_id))
            .exec(transaction)
            .await
            .db()?;
        let relations = item
            .permission_codes
            .iter()
            .map(|code| {
                let perm_id = permission_ids
                    .get(&normalize_stable_key(code))
                    .copied()
                    .ok_or_else(|| AppError::Conflict(format!("角色引用的权限 {code} 不存在")))?;
                Ok(role_permission::ActiveModel::from(role_permission::Model {
                    tenant_id: tenant_id.to_owned(),
                    role_id,
                    perm_id,
                }))
            })
            .collect::<AppResult<Vec<_>>>()?;
        if !relations.is_empty() {
            role_permission::Entity::insert_many(relations)
                .exec(transaction)
                .await
                .db()?;
        }
        role_dept::Entity::delete_many()
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .filter(role_dept::Column::RoleId.eq(role_id))
            .exec(transaction)
            .await
            .db()?;
        let departments = item
            .custom_department_paths
            .iter()
            .map(|path| {
                let dept_id = department_ids
                    .get(&normalize_department_path(path))
                    .copied()
                    .ok_or_else(|| {
                        AppError::Conflict(format!("角色引用的部门路径 {} 不存在", join_path(path)))
                    })?;
                Ok(role_dept::ActiveModel::from(role_dept::Model {
                    tenant_id: tenant_id.to_owned(),
                    role_id,
                    dept_id,
                }))
            })
            .collect::<AppResult<Vec<_>>>()?;
        if !departments.is_empty() {
            role_dept::Entity::insert_many(departments)
                .exec(transaction)
                .await
                .db()?;
        }
    }
    Ok(())
}
