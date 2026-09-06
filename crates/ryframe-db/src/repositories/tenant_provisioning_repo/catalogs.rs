use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use ryframe_application::ports::tenants::{
    TenantBaseCatalogTemplate, TenantConfigTemplate, TenantDepartmentTemplate,
    TenantDictionaryDataTemplate, TenantDictionaryTypeTemplate, TenantPostTemplate,
    TenantProvisioningIdentity,
};
use ryframe_kernel::{AppError, AppResult};
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
    let departments = validate_and_order_departments(departments)?;
    let mut department_ids = HashMap::new();
    for source in departments {
        let id = crate::next_id()?;
        let (parent_id, ancestors) = remap_department_references(&source, &department_ids)?;
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

fn validate_and_order_departments(
    departments: Vec<TenantDepartmentTemplate>,
) -> AppResult<Vec<TenantDepartmentTemplate>> {
    let mut source_ids = HashSet::with_capacity(departments.len());
    for source in &departments {
        if source.source_id == 0 {
            return Err(AppError::Internal(
                "复制租户部门目录失败：源部门 ID 不能使用根哨兵 0".into(),
            ));
        }
        if !source_ids.insert(source.source_id) {
            return Err(AppError::Internal(format!(
                "复制租户部门目录失败：源部门 ID {} 重复",
                source.source_id
            )));
        }
    }
    for source in &departments {
        if let Some(parent_source_id) = source.parent_source_id
            && !source_ids.contains(&parent_source_id)
        {
            return Err(missing_department_reference(
                source.source_id,
                "父部门",
                parent_source_id,
            ));
        }
    }

    order_departments(departments)
}

fn order_departments(
    mut remaining: Vec<TenantDepartmentTemplate>,
) -> AppResult<Vec<TenantDepartmentTemplate>> {
    let total = remaining.len();
    let mut ordered = Vec::with_capacity(total);
    let mut source_paths = HashMap::<i64, Vec<i64>>::with_capacity(total);
    while !remaining.is_empty() {
        let mut progressed = false;
        let mut index = 0;
        while index < remaining.len() {
            let parent_ready = remaining[index]
                .parent_source_id
                .is_none_or(|parent_source_id| source_paths.contains_key(&parent_source_id));
            if !parent_ready {
                index += 1;
                continue;
            }
            let source = remaining.remove(index);
            let expected_path = expected_department_path(&source, &source_paths)?;
            validate_department_path(&source, &expected_path)?;
            source_paths.insert(source.source_id, expected_path);
            ordered.push(source);
            progressed = true;
        }
        if !progressed {
            let mut source_ids = remaining
                .iter()
                .map(|source| source.source_id)
                .collect::<Vec<_>>();
            source_ids.sort_unstable();
            return Err(AppError::Internal(format!(
                "复制租户部门目录失败：父级关系存在环，涉及源部门 {source_ids:?}"
            )));
        }
    }
    Ok(ordered)
}

fn expected_department_path(
    source: &TenantDepartmentTemplate,
    source_paths: &HashMap<i64, Vec<i64>>,
) -> AppResult<Vec<i64>> {
    match source.parent_source_id {
        None => Ok(vec![0]),
        Some(parent_source_id) => {
            let mut path = source_paths
                .get(&parent_source_id)
                .cloned()
                .ok_or_else(|| {
                    missing_department_reference(source.source_id, "父部门", parent_source_id)
                })?;
            path.push(parent_source_id);
            Ok(path)
        }
    }
}

fn validate_department_path(
    source: &TenantDepartmentTemplate,
    expected_path: &[i64],
) -> AppResult<()> {
    let has_one_leading_root = source.ancestor_source_ids.first() == Some(&0)
        && source
            .ancestor_source_ids
            .iter()
            .filter(|source_id| **source_id == 0)
            .count()
            == 1;
    if !has_one_leading_root {
        return Err(AppError::Internal(format!(
            "复制租户部门目录失败：源部门 {} 的祖级列表必须以唯一根哨兵 0 开头",
            source.source_id
        )));
    }
    if source.ancestor_source_ids != expected_path {
        return Err(AppError::Internal(format!(
            "复制租户部门目录失败：源部门 {} 的祖级列表与父级链不一致",
            source.source_id
        )));
    }
    Ok(())
}

fn remap_department_references(
    source: &TenantDepartmentTemplate,
    department_ids: &HashMap<i64, i64>,
) -> AppResult<(Option<i64>, String)> {
    let parent_id = source
        .parent_source_id
        .map(|parent_source_id| {
            department_ids
                .get(&parent_source_id)
                .copied()
                .ok_or_else(|| {
                    missing_department_reference(source.source_id, "父部门", parent_source_id)
                })
        })
        .transpose()?;
    let ancestors = source
        .ancestor_source_ids
        .iter()
        .map(|source_id| {
            if *source_id == 0 {
                return Ok("0".to_owned());
            }
            department_ids
                .get(source_id)
                .map(ToString::to_string)
                .ok_or_else(|| {
                    missing_department_reference(source.source_id, "祖级部门", *source_id)
                })
        })
        .collect::<AppResult<Vec<_>>>()?
        .join(",");
    Ok((parent_id, ancestors))
}

fn missing_department_reference(
    source_id: i64,
    reference_kind: &str,
    reference_source_id: i64,
) -> AppError {
    AppError::Internal(format!(
        "复制租户部门目录失败：源部门 {source_id} 的{reference_kind} {reference_source_id} 尚未映射"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unordered_three_level_departments_are_copied_with_remapped_paths() {
        let departments = vec![
            department(3, Some(2), vec![0, 1, 2]),
            department(1, None, vec![0]),
            department(2, Some(1), vec![0, 1]),
        ];
        let ordered = validate_and_order_departments(departments).unwrap();
        assert_eq!(
            ordered
                .iter()
                .map(|source| source.source_id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        let mut target_ids = HashMap::new();
        for (source, target_id) in ordered.iter().zip([101, 102, 103]) {
            let references = remap_department_references(source, &target_ids).unwrap();
            let expected = match source.source_id {
                1 => (None, "0"),
                2 => (Some(101), "0,101"),
                3 => (Some(102), "0,101,102"),
                _ => unreachable!(),
            };
            assert_eq!(references, (expected.0, expected.1.to_owned()));
            target_ids.insert(source.source_id, target_id);
        }
    }

    #[test]
    fn department_graph_rejects_missing_parent_and_cycle() {
        let missing_parent = department(2, Some(1), vec![0, 1]);
        assert!(matches!(
            validate_and_order_departments(vec![missing_parent]),
            Err(AppError::Internal(message)) if message.contains("父部门 1 尚未映射")
        ));

        assert!(matches!(
            validate_and_order_departments(vec![
                department(1, Some(2), vec![0, 2]),
                department(2, Some(1), vec![0, 1]),
            ]),
            Err(AppError::Internal(message)) if message.contains("父级关系存在环")
        ));
    }

    #[test]
    fn department_graph_rejects_inconsistent_path_or_root_sentinel() {
        assert!(matches!(
            validate_and_order_departments(vec![
                department(1, None, vec![0]),
                department(2, Some(1), vec![0, 99]),
            ]),
            Err(AppError::Internal(message)) if message.contains("祖级列表与父级链不一致")
        ));
        for path in [vec![], vec![1], vec![0, 0]] {
            assert!(matches!(
                validate_and_order_departments(vec![department(1, None, path)]),
                Err(AppError::Internal(message))
                    if message.contains("必须以唯一根哨兵 0 开头")
            ));
        }

        let missing_ancestor = department(3, None, vec![0, 2]);
        assert!(matches!(
            remap_department_references(&missing_ancestor, &HashMap::new()),
            Err(AppError::Internal(message)) if message.contains("祖级部门 2 尚未映射")
        ));
    }

    fn department(
        source_id: i64,
        parent_source_id: Option<i64>,
        ancestor_source_ids: Vec<i64>,
    ) -> TenantDepartmentTemplate {
        TenantDepartmentTemplate {
            source_id,
            name: format!("部门-{source_id}"),
            parent_source_id,
            ancestor_source_ids,
            sort: 0,
            status: "1".to_owned(),
            remark: None,
            delete_flag: "0".to_owned(),
        }
    }
}
