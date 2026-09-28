use super::*;

pub(super) async fn upsert_posts(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &TenantConfigPackageResources,
    changed: &BTreeSet<(String, String)>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    for item in &resources.posts {
        if !changed.contains(&("post".to_owned(), normalize_stable_key(&item.code))) {
            continue;
        }
        let existing = post::Entity::find()
            .filter(post::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .db()?
            .into_iter()
            .find(|candidate| {
                normalize_stable_key(&candidate.code) == normalize_stable_key(&item.code)
            });
        let model = post::Model {
            id: existing.as_ref().map(|item| item.id).unwrap_or(next_id()?),
            tenant_id: tenant_id.to_owned(),
            name: item.name.clone(),
            code: item.code.clone(),
            sort: item.sort,
            status: item.status.clone(),
            remark: item.remark.clone(),
            del_flag: post::SOFT_DELETE_ACTIVE.to_owned(),
            created_at: existing.as_ref().map(|item| item.created_at).unwrap_or(now),
            updated_at: now,
        };
        save_model(
            transaction,
            existing.is_some(),
            post::ActiveModel::from(model),
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn upsert_dict_types(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &TenantConfigPackageResources,
    changed: &BTreeSet<(String, String)>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    for item in &resources.dict_types {
        if !changed.contains(&("dict_type".to_owned(), normalize_stable_key(&item.code))) {
            continue;
        }
        let existing = dict_type::Entity::find()
            .filter(dict_type::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .db()?
            .into_iter()
            .find(|candidate| {
                normalize_stable_key(&candidate.code) == normalize_stable_key(&item.code)
            });
        let model = dict_type::Model {
            id: existing.as_ref().map(|item| item.id).unwrap_or(next_id()?),
            tenant_id: tenant_id.to_owned(),
            name: item.name.clone(),
            code: item.code.clone(),
            status: item.status.clone(),
            remark: item.remark.clone(),
            del_flag: dict_type::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: existing.as_ref().map(|item| item.created_at).unwrap_or(now),
            updated_at: now,
        };
        save_model(
            transaction,
            existing.is_some(),
            dict_type::ActiveModel::from(model),
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn upsert_dict_data(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &TenantConfigPackageResources,
    changed: &BTreeSet<(String, String)>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    for item in &resources.dict_data {
        let key = format!("{}:{}:{}", item.type_code.len(), item.type_code, item.value);
        if !changed.contains(&("dict_data".to_owned(), normalize_stable_key(&key))) {
            continue;
        }
        let existing = dict_data::Entity::find()
            .filter(dict_data::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .db()?
            .into_iter()
            .find(|candidate| {
                normalize_stable_key(&candidate.type_code) == normalize_stable_key(&item.type_code)
                    && normalize_stable_key(&candidate.value) == normalize_stable_key(&item.value)
            });
        let model = dict_data::Model {
            id: existing.as_ref().map(|item| item.id).unwrap_or(next_id()?),
            tenant_id: tenant_id.to_owned(),
            type_code: item.type_code.clone(),
            label: item.label.clone(),
            value: item.value.clone(),
            sort: item.sort,
            status: item.status.clone(),
            css_class: item.css_class.clone(),
            remark: item.remark.clone(),
            del_flag: dict_data::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: existing.as_ref().map(|item| item.created_at).unwrap_or(now),
            updated_at: now,
        };
        save_model(
            transaction,
            existing.is_some(),
            dict_data::ActiveModel::from(model),
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn upsert_configs(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &TenantConfigPackageResources,
    changed: &BTreeSet<(String, String)>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    for item in &resources.configs {
        if !changed.contains(&("config".to_owned(), normalize_stable_key(&item.key))) {
            continue;
        }
        if ryframe_application::system::platform::is_sensitive_config_key(&item.key) {
            return Err(AppError::Validation("敏感参数不能应用".into()));
        }
        let existing = config::Entity::find()
            .filter(config::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .db()?
            .into_iter()
            .find(|candidate| {
                normalize_stable_key(&candidate.key) == normalize_stable_key(&item.key)
            });
        let model = config::Model {
            id: existing.as_ref().map(|item| item.id).unwrap_or(next_id()?),
            tenant_id: tenant_id.to_owned(),
            name: item.name.clone(),
            key: item.key.clone(),
            value: item.value.clone(),
            portable: true,
            remark: item.remark.clone(),
            del_flag: config::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: existing.as_ref().map(|item| item.created_at).unwrap_or(now),
            updated_at: now,
        };
        save_model(
            transaction,
            existing.is_some(),
            config::ActiveModel::from(model),
        )
        .await?;
    }
    Ok(())
}
