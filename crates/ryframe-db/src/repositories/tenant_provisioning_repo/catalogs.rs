use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ryframe_application::ports::tenants::{
    TenantBaseCatalogTemplate, TenantConfigTemplate, TenantDepartmentTemplate,
    TenantDictionaryDataTemplate, TenantDictionaryTypeTemplate, TenantPostTemplate,
    TenantProvisioningIdentity,
};
use ryframe_kernel::AppResult;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseTransaction, EntityTrait};

use super::TenantProvisioningRepository;
use crate::{
    DbResultExt,
    entities::{config, dept, dict_data, dict_type},
    generated::entities::post,
};

impl TenantProvisioningRepository {
    pub async fn copy_base_catalogs_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        identity: &TenantProvisioningIdentity,
        template: TenantBaseCatalogTemplate,
    ) -> AppResult<()> {
        let now = identity.provisioned_at;
        copy_posts(transaction, &identity.tenant_id, template.posts, now).await?;
        copy_configs(transaction, &identity.tenant_id, template.configs, now).await?;
        copy_dictionaries(
            transaction,
            &identity.tenant_id,
            template.dictionary_types,
            template.dictionary_data,
            now,
        )
        .await?;
        copy_departments(transaction, &identity.tenant_id, template.departments, now).await
    }
}

async fn copy_posts(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    posts: Vec<TenantPostTemplate>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let models = posts
        .into_iter()
        .map(|source| -> AppResult<_> {
            Ok(post::ActiveModel {
                id: ActiveValue::Set(crate::next_id()?),
                tenant_id: ActiveValue::Set(tenant_id.to_owned()),
                name: ActiveValue::Set(source.name),
                code: ActiveValue::Set(source.code),
                sort: ActiveValue::Set(source.sort),
                status: ActiveValue::Set(source.status),
                remark: ActiveValue::Set(source.remark),
                del_flag: ActiveValue::Set(source.delete_flag),
                created_at: ActiveValue::Set(now),
                updated_at: ActiveValue::Set(now),
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    if !models.is_empty() {
        post::Entity::insert_many(models)
            .exec(transaction)
            .await
            .db()?;
    }
    Ok(())
}

async fn copy_configs(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    configs: Vec<TenantConfigTemplate>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let models = configs
        .into_iter()
        .map(|source| -> AppResult<_> {
            Ok(config::ActiveModel {
                id: ActiveValue::Set(crate::next_id()?),
                tenant_id: ActiveValue::Set(tenant_id.to_owned()),
                name: ActiveValue::Set(source.name),
                key: ActiveValue::Set(source.key),
                value: ActiveValue::Set(source.value),
                portable: ActiveValue::Set(source.portable),
                remark: ActiveValue::Set(source.remark),
                del_flag: ActiveValue::Set(source.delete_flag),
                created_at: ActiveValue::Set(now),
                updated_at: ActiveValue::Set(now),
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    if !models.is_empty() {
        config::Entity::insert_many(models)
            .exec(transaction)
            .await
            .db()?;
    }
    Ok(())
}

async fn copy_dictionaries(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    dictionary_types: Vec<TenantDictionaryTypeTemplate>,
    dictionary_data: Vec<TenantDictionaryDataTemplate>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let types = dictionary_types
        .into_iter()
        .map(|source| -> AppResult<_> {
            Ok(dict_type::ActiveModel {
                id: ActiveValue::Set(crate::next_id()?),
                tenant_id: ActiveValue::Set(tenant_id.to_owned()),
                name: ActiveValue::Set(source.name),
                code: ActiveValue::Set(source.code),
                status: ActiveValue::Set(source.status),
                remark: ActiveValue::Set(source.remark),
                del_flag: ActiveValue::Set(source.delete_flag),
                created_at: ActiveValue::Set(now),
                updated_at: ActiveValue::Set(now),
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    if !types.is_empty() {
        dict_type::Entity::insert_many(types)
            .exec(transaction)
            .await
            .db()?;
    }
    let data = dictionary_data
        .into_iter()
        .map(|source| -> AppResult<_> {
            Ok(dict_data::ActiveModel {
                id: ActiveValue::Set(crate::next_id()?),
                tenant_id: ActiveValue::Set(tenant_id.to_owned()),
                type_code: ActiveValue::Set(source.type_code),
                label: ActiveValue::Set(source.label),
                value: ActiveValue::Set(source.value),
                sort: ActiveValue::Set(source.sort),
                status: ActiveValue::Set(source.status),
                css_class: ActiveValue::Set(source.css_class),
                remark: ActiveValue::Set(source.remark),
                del_flag: ActiveValue::Set(source.delete_flag),
                created_at: ActiveValue::Set(now),
                updated_at: ActiveValue::Set(now),
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    if !data.is_empty() {
        dict_data::Entity::insert_many(data)
            .exec(transaction)
            .await
            .db()?;
    }
    Ok(())
}

async fn copy_departments(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    departments: Vec<TenantDepartmentTemplate>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let mut department_ids = HashMap::new();
    for source in departments {
        let id = crate::next_id()?;
        let parent_id = source
            .parent_source_id
            .and_then(|parent| department_ids.get(&parent).copied());
        let ancestors = source
            .ancestor_source_ids
            .iter()
            .filter_map(|source_id| department_ids.get(source_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        dept::ActiveModel {
            id: ActiveValue::Set(id),
            tenant_id: ActiveValue::Set(tenant_id.to_owned()),
            name: ActiveValue::Set(source.name),
            parent_id: ActiveValue::Set(parent_id),
            ancestors: ActiveValue::Set(ancestors),
            sort: ActiveValue::Set(source.sort),
            status: ActiveValue::Set(source.status),
            remark: ActiveValue::Set(source.remark),
            del_flag: ActiveValue::Set(source.delete_flag),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(transaction)
        .await
        .db()?;
        department_ids.insert(source.source_id, id);
    }
    Ok(())
}
