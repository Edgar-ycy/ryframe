use ryframe_application::ports::tenants::{
    TenantAuthorizationTemplate, TenantBaseCatalogTemplate, TenantConfigTemplate,
    TenantDepartmentTemplate, TenantDictionaryDataTemplate, TenantDictionaryTypeTemplate,
    TenantMenuTemplate, TenantPermissionTemplate, TenantPostTemplate, TenantProvisioningTemplate,
};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter, QueryOrder};

use super::TenantProvisioningRepository;
use crate::{
    DbResultExt,
    entities::{config, dept, dict_data, dict_type, menu, permission},
    generated::entities::post,
};

const TEMPLATE_TENANT_ID: &str = "system";
const PLATFORM_PERMISSION_PREFIX: &str = "platform:";

impl TenantProvisioningRepository {
    pub async fn load_template_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
    ) -> AppResult<TenantProvisioningTemplate> {
        // 保持原开通流程的查询顺序，避免事务内读取行为随模块拆分漂移。
        let menus = load_menus(transaction).await?;
        let base_catalogs = load_base_catalogs(transaction).await?;
        let permissions = load_permissions(transaction).await?;
        Ok(TenantProvisioningTemplate {
            authorization: TenantAuthorizationTemplate { permissions, menus },
            base_catalogs,
        })
    }
}

async fn load_menus(transaction: &DatabaseTransaction) -> AppResult<Vec<TenantMenuTemplate>> {
    Ok(menu::Entity::find()
        .filter(menu::Column::TenantId.eq(TEMPLATE_TENANT_ID))
        .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
        .order_by_asc(menu::Column::Id)
        .all(transaction)
        .await
        .db()?
        .into_iter()
        .map(map_menu)
        .collect())
}

async fn load_permissions(
    transaction: &DatabaseTransaction,
) -> AppResult<Vec<TenantPermissionTemplate>> {
    Ok(permission::Entity::find()
        .filter(permission::Column::TenantId.eq(TEMPLATE_TENANT_ID))
        .filter(permission::Column::Code.ne("*:*:*"))
        .filter(permission::Column::Code.not_like("tenant:%"))
        .filter(permission::Column::Code.not_like(format!("{PLATFORM_PERMISSION_PREFIX}%")))
        .filter(permission::Column::Code.not_like("monitor:retention:%"))
        .order_by_asc(permission::Column::Id)
        .all(transaction)
        .await
        .db()?
        .into_iter()
        .map(map_permission)
        .collect())
}

async fn load_base_catalogs(
    transaction: &DatabaseTransaction,
) -> AppResult<TenantBaseCatalogTemplate> {
    let posts = post::Entity::find()
        .filter(post::Column::TenantId.eq(TEMPLATE_TENANT_ID))
        .filter(post::Column::DelFlag.eq(post::SOFT_DELETE_ACTIVE))
        .all(transaction)
        .await
        .db()?
        .into_iter()
        .map(map_post)
        .collect();
    let configs = config::Entity::find()
        .filter(config::Column::TenantId.eq(TEMPLATE_TENANT_ID))
        .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .db()?
        .into_iter()
        .map(map_config)
        .collect();
    let dictionary_types = load_dictionary_types(transaction).await?;
    let dictionary_data = load_dictionary_data(transaction).await?;
    let departments = load_departments(transaction).await?;
    Ok(TenantBaseCatalogTemplate {
        posts,
        configs,
        dictionary_types,
        dictionary_data,
        departments,
    })
}

async fn load_dictionary_types(
    transaction: &DatabaseTransaction,
) -> AppResult<Vec<TenantDictionaryTypeTemplate>> {
    dict_type::Entity::find()
        .filter(dict_type::Column::TenantId.eq(TEMPLATE_TENANT_ID))
        .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .db()
        .map(|items| items.into_iter().map(map_dictionary_type).collect())
}

async fn load_dictionary_data(
    transaction: &DatabaseTransaction,
) -> AppResult<Vec<TenantDictionaryDataTemplate>> {
    dict_data::Entity::find()
        .filter(dict_data::Column::TenantId.eq(TEMPLATE_TENANT_ID))
        .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .db()
        .map(|items| items.into_iter().map(map_dictionary_data).collect())
}

async fn load_departments(
    transaction: &DatabaseTransaction,
) -> AppResult<Vec<TenantDepartmentTemplate>> {
    let departments = dept::Entity::find()
        .filter(dept::Column::TenantId.eq(TEMPLATE_TENANT_ID))
        .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
        .order_by_asc(dept::Column::Id)
        .all(transaction)
        .await
        .db()?;
    departments.into_iter().map(map_department).collect()
}

fn map_permission(source: permission::Model) -> TenantPermissionTemplate {
    TenantPermissionTemplate {
        source_id: source.id,
        name: source.name,
        code: source.code,
        parent_source_id: source.parent_id,
        permission_type: source.perm_type,
        icon: source.icon,
        sort: source.sort,
        status: source.status,
        assign_admin: false,
        assign_user: false,
    }
}

fn map_menu(source: menu::Model) -> TenantMenuTemplate {
    TenantMenuTemplate {
        source_id: source.id,
        name: source.name,
        parent_source_id: source.parent_id,
        menu_type: source.menu_type,
        permission_source_id: source.perm_id,
        route_key: source.route_key,
        icon: source.icon,
        sort: source.sort,
        visible: source.visible,
        status: source.status,
        remark: source.remark,
        delete_flag: source.del_flag,
    }
}

fn map_post(source: post::Model) -> TenantPostTemplate {
    TenantPostTemplate {
        name: source.name,
        code: source.code,
        sort: source.sort,
        status: source.status,
        remark: source.remark,
        delete_flag: source.del_flag,
    }
}

fn map_config(source: config::Model) -> TenantConfigTemplate {
    TenantConfigTemplate {
        name: source.name,
        key: source.key,
        value: source.value,
        portable: source.portable,
        remark: source.remark,
        delete_flag: source.del_flag,
    }
}

fn map_dictionary_type(source: dict_type::Model) -> TenantDictionaryTypeTemplate {
    TenantDictionaryTypeTemplate {
        name: source.name,
        code: source.code,
        status: source.status,
        remark: source.remark,
        delete_flag: source.del_flag,
    }
}

fn map_dictionary_data(source: dict_data::Model) -> TenantDictionaryDataTemplate {
    TenantDictionaryDataTemplate {
        type_code: source.type_code,
        label: source.label,
        value: source.value,
        sort: source.sort,
        status: source.status,
        css_class: source.css_class,
        remark: source.remark,
        delete_flag: source.del_flag,
    }
}

fn map_department(source: dept::Model) -> AppResult<TenantDepartmentTemplate> {
    let ancestor_source_ids = parse_department_ancestors(source.id, &source.ancestors)?;
    Ok(TenantDepartmentTemplate {
        source_id: source.id,
        name: source.name,
        parent_source_id: source.parent_id,
        ancestor_source_ids,
        sort: source.sort,
        status: source.status,
        remark: source.remark,
        delete_flag: source.del_flag,
    })
}

fn parse_department_ancestors(source_id: i64, ancestors: &str) -> AppResult<Vec<i64>> {
    ancestors
        .split(',')
        .map(|part| {
            let token = part.trim();
            if token.is_empty() {
                return Err(AppError::Internal(format!(
                    "复制租户部门目录失败：源部门 {source_id} 的祖级列表包含空 token"
                )));
            }
            token.parse::<i64>().map_err(|_| {
                AppError::Internal(format!(
                    "复制租户部门目录失败：源部门 {source_id} 的祖级 token `{token}` 不是有效 ID"
                ))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn department_ancestor_parser_rejects_invalid_or_empty_token() {
        assert_eq!(
            parse_department_ancestors(3, "0, 1,2").unwrap(),
            vec![0, 1, 2]
        );
        assert!(matches!(
            parse_department_ancestors(3, "0,invalid"),
            Err(AppError::Internal(message)) if message.contains("`invalid` 不是有效 ID")
        ));
        assert!(matches!(
            parse_department_ancestors(3, "0,,2"),
            Err(AppError::Internal(message)) if message.contains("包含空 token")
        ));
    }
}
