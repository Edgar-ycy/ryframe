use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    BusinessPackage, OwnedResourceDescriptor, model::descriptor_to_ir, writer::BusinessAsset,
};
use crate::{ResourceError, ResourceIr};

mod diff;
mod files;
mod render;
pub(super) use render::registry;

pub(super) const HISTORY_PATH: &str = "src/generated/migrations/history.json";

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct History {
    format_version: u16,
    module: String,
    resources: Vec<ResourceHistory>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResourceHistory {
    name: String,
    descriptor: OwnedResourceDescriptor,
    versions: Vec<Version>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Version {
    id: String,
    checksum: String,
    schema_hash: String,
    canonical: String,
}

pub(super) fn prepare(
    package: &BusinessPackage,
    resources: &[ResourceIr],
    descriptors: &[OwnedResourceDescriptor],
    assets: &mut Vec<BusinessAsset>,
) -> Result<History, ResourceError> {
    let mut previous = files::load(package)?;
    if previous
        .resources
        .iter()
        .any(|old| !resources.iter().any(|resource| resource.name == old.name))
    {
        return Err(error(
            "已有资源被删除或改名，需要先审阅数据及迁移归属；本批未写入",
        ));
    }
    let mut history = History {
        format_version: 1,
        module: package.module.clone(),
        resources: Vec::new(),
    };
    for (resource, descriptor) in resources.iter().zip(descriptors) {
        let old = previous
            .resources
            .iter()
            .position(|old| old.name == resource.name)
            .map(|index| previous.resources.remove(index));
        history.resources.push(prepare_resource(
            package, resource, descriptor, old, assets,
        )?);
    }
    validate(&history, &package.module)?;
    assets.push(BusinessAsset::new(
        "__catalog__",
        HISTORY_PATH,
        serde_json::to_string_pretty(&history)
            .map_err(|cause| error(format!("迁移历史序列化失败：{cause}")))?
            + "\n",
    ));
    Ok(history)
}

fn prepare_resource(
    package: &BusinessPackage,
    resource: &ResourceIr,
    descriptor: &OwnedResourceDescriptor,
    old: Option<ResourceHistory>,
    assets: &mut Vec<BusinessAsset>,
) -> Result<ResourceHistory, ResourceError> {
    let initial = format!("m_resource_initial_{}", resource.name);
    let path = format!("migrations/{initial}.rs");
    let versions = if let Some(old) = old {
        let before = descriptor_to_ir(package, &old.descriptor)?;
        let latest = old
            .versions
            .last()
            .ok_or_else(|| error("迁移历史没有初始版本"))?;
        if crate::resource::canonical_schema(&before) != latest.canonical {
            return Err(error(format!(
                "资源 {} 的已登记模型与迁移历史末态不一致，拒绝猜测基线",
                resource.name
            )));
        }
        preserve(package, &resource.name, &old.versions, assets)?;
        if crate::resource::canonical_schema(resource) == latest.canonical
            && column_comments(&before) == column_comments(resource)
        {
            old.versions
        } else {
            let statements = diff::statements(&before, resource)?;
            let id = revision_id(&resource.name, old.versions.len(), &resource.schema_hash)?;
            let mut added = vec![BusinessAsset::new(
                &resource.name,
                format!("migrations/{id}.rs"),
                render::incremental(resource, &id, &statements),
            )];
            super::format::format_assets(&package.workspace_root, &mut added)?;
            let version = Version {
                id,
                checksum: hash(added[0].content.as_bytes()),
                schema_hash: resource.schema_hash.clone(),
                canonical: crate::resource::canonical_schema(resource),
            };
            assets.extend(added);
            let mut versions = old.versions;
            versions.push(version);
            versions
        }
    } else {
        let source = assets
            .iter()
            .find(|asset| asset.path == path)
            .ok_or_else(|| error(format!("资源 {} 缺少初始迁移", resource.name)))?;
        vec![Version {
            id: initial,
            checksum: hash(source.content.as_bytes()),
            schema_hash: resource.schema_hash.clone(),
            canonical: crate::resource::canonical_schema(resource),
        }]
    };
    Ok(ResourceHistory {
        name: resource.name.clone(),
        descriptor: descriptor.clone(),
        versions,
    })
}

fn preserve(
    package: &BusinessPackage,
    name: &str,
    versions: &[Version],
    assets: &mut Vec<BusinessAsset>,
) -> Result<(), ResourceError> {
    for version in versions {
        let path = format!("migrations/{}.rs", version.id);
        let source = files::migration(package, version)?;
        if let Some(current) = assets.iter_mut().find(|asset| asset.path == path) {
            current.content = source;
        } else {
            assets.push(BusinessAsset::new(name, path, source));
        }
    }
    Ok(())
}

fn column_comments(resource: &ResourceIr) -> Vec<(&str, &str)> {
    resource
        .fields
        .iter()
        .map(|field| {
            (
                field.column.as_str(),
                field.sql_comment.as_deref().unwrap_or_default(),
            )
        })
        .collect()
}

fn revision_id(resource: &str, index: usize, schema: &str) -> Result<String, ResourceError> {
    if index > 999_999 || !valid_hash(schema) {
        return Err(error("迁移序号或模型摘要不合法"));
    }
    let id = if index == 0 {
        format!("m_resource_initial_{resource}")
    } else {
        format!(
            "m_resource_initial_{resource}_r{index:06}_{}",
            &schema[..12]
        )
    };
    if id.len() > 128 {
        return Err(error("业务迁移标识超过账本长度，缩短资源名称"));
    }
    Ok(id)
}

fn validate(history: &History, module: &str) -> Result<(), ResourceError> {
    if history.format_version != 1 || history.module != module {
        return Err(error("迁移历史版本或模块归属不匹配"));
    }
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for resource in &history.resources {
        if resource.name != resource.descriptor.name
            || !names.insert(&resource.name)
            || resource.versions.is_empty()
            || resource.name.is_empty()
            || !resource
                .name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(error("迁移历史资源名称、归属或初始版本无效"));
        }
        for (index, version) in resource.versions.iter().enumerate() {
            if !valid_hash(&version.checksum)
                || !valid_hash(&version.schema_hash)
                || version.id != revision_id(&resource.name, index, &version.schema_hash)?
                || !ids.insert(&version.id)
            {
                return Err(error("迁移历史标识、顺序或摘要无效，拒绝读取未知路径"));
            }
        }
    }
    Ok(())
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn hash(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}
fn error(message: impl Into<String>) -> ResourceError {
    ResourceError::new(
        message,
        "从版本控制核对完整迁移、history.json 和 ownership；保留旧迁移，修正模型后重新预览",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::IndexIr;
    use crate::{StorageKind, ValueType};

    fn resource() -> ResourceIr {
        crate::load_resource(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/order.toml"),
        )
        .unwrap()
    }

    #[test]
    fn incremental_diff_preserves_column_positions_defaults_and_index_names() {
        let before = resource();
        let mut after = before.clone();
        let mut added = after
            .fields
            .iter()
            .find(|field| field.name == "name")
            .unwrap()
            .clone();
        added.name = "memo".into();
        added.column = "memo_text".into();
        added.nullable = true;
        added.default = None;
        after.fields.insert(3, added);
        after
            .fields
            .iter_mut()
            .find(|field| field.name == "name")
            .unwrap()
            .validation
            .max_length = Some(120);
        after.indexes.push(IndexIr {
            name: "idx_order_memo".into(),
            fields: vec!["memo".into()],
            unique: false,
        });
        let sql = diff::statements(&before, &after).unwrap().join("\n");
        assert!(sql.contains("MODIFY COLUMN `name` VARCHAR(120) NOT NULL"));
        assert!(sql.contains("ADD COLUMN `memo_text` VARCHAR(100) NULL AFTER `name`"));
        assert!(sql.contains("ADD KEY `idx_order_memo` (`memo_text`)"));
        assert!(!sql.contains("DROP") && !sql.contains("CREATE TABLE"));
    }

    #[test]
    fn incremental_diff_reports_every_risky_change_and_never_silently_narrows() {
        let before = resource();
        let mut after = before.clone();
        after
            .fields
            .iter_mut()
            .find(|field| field.name == "name")
            .unwrap()
            .validation
            .max_length = Some(20);
        after.fields.retain(|field| field.name != "updated_at");
        after
            .indexes
            .retain(|index| index.name != "uk_order_tenant_name");
        after.foreign_keys.push(crate::ForeignKeySpec {
            name: "fk_order_parent".into(),
            columns: vec!["tenant_id".into(), "id".into()],
            referenced_table: "biz_parent".into(),
            referenced_columns: vec!["tenant_id".into(), "id".into()],
            update_rule: crate::import::ForeignKeyAction::Restrict,
            delete_rule: crate::import::ForeignKeyAction::Restrict,
            match_option: crate::import::ForeignKeyMatch::None,
        });
        let error = diff::statements(&before, &after).unwrap_err().to_string();
        for detail in [
            "name",
            "updated_at",
            "uk_order_tenant_name",
            "物理外键",
            "人工审阅",
            "本批未写入",
        ] {
            assert!(error.contains(detail), "{error}");
        }
    }

    #[test]
    fn incremental_diff_rejects_scope_primary_id_strategy_reordering_and_nonnull_addition() {
        let before = resource();
        let mut after = before.clone();
        after.storage = StorageKind::ControlRow;
        after.primary_key.reverse();
        after.fields.swap(2, 3);
        let mut added = after.fields[2].clone();
        added.name = "required_field".into();
        added.column = "required_field".into();
        added.default = None;
        added.nullable = false;
        after.fields.push(added);
        let error = diff::statements(&before, &after).unwrap_err().to_string();
        for detail in ["物理主键", "已有列顺序", "required_field", "没有显式默认值"]
        {
            assert!(error.contains(detail), "{error}");
        }
    }

    #[test]
    fn integer_widening_and_nullable_relaxation_are_forward_changes() {
        let before = resource();
        let mut after = before.clone();
        let status = after
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap();
        status.value_type = ValueType::I64;
        status.nullable = true;
        let sql = diff::statements(&before, &after).unwrap().join("\n");
        assert!(sql.contains("MODIFY COLUMN `status` BIGINT NULL DEFAULT 1"));
        assert!(
            diff::statements(&after, &before)
                .unwrap_err()
                .to_string()
                .contains("收紧非空约束")
        );
    }

    #[test]
    fn generated_revision_ids_sort_after_initial_and_reject_invalid_hashes() {
        let initial = revision_id("order", 0, &"a".repeat(64)).unwrap();
        let first = revision_id("order", 1, &"b".repeat(64)).unwrap();
        let second = revision_id("order", 2, &"c".repeat(64)).unwrap();
        assert!(initial < first && first < second);
        assert!(revision_id("order", 1, "../invalid").is_err());
        assert!(revision_id("order", 1_000_000, &"a".repeat(64)).is_err());
    }

    #[test]
    fn physical_text_to_varchar_requires_review_but_varchar_widening_is_safe() {
        let mut before = resource();
        before
            .fields
            .iter_mut()
            .find(|field| field.name == "name")
            .unwrap()
            .sql_type = Some("TEXT".into());
        let mut after = before.clone();
        after
            .fields
            .iter_mut()
            .find(|field| field.name == "name")
            .unwrap()
            .sql_type = Some("VARCHAR(1000)".into());
        assert!(
            diff::statements(&before, &after)
                .unwrap_err()
                .to_string()
                .contains("人工审阅")
        );
        before
            .fields
            .iter_mut()
            .find(|field| field.name == "name")
            .unwrap()
            .sql_type = Some("VARCHAR(32)".into());
        let sql = diff::statements(&before, &after).unwrap().join("\n");
        assert!(sql.contains("MODIFY COLUMN `name` VARCHAR(1000)"));
    }

    #[test]
    fn collation_changes_require_review_even_when_column_type_is_unchanged() {
        let before = resource();
        let mut after = before.clone();
        after.collation = Some("utf8mb4_0900_ai_ci".into());
        let error = diff::statements(&before, &after).unwrap_err().to_string();
        assert!(error.contains("表排序规则变化") && error.contains("本批未写入"));
        assert!(!error.contains("列 id 排序规则变化"));
        let mut after = before.clone();
        after
            .fields
            .iter_mut()
            .find(|field| field.name == "name")
            .unwrap()
            .collation = Some("utf8mb4_bin".into());
        assert!(
            diff::statements(&before, &after)
                .unwrap_err()
                .to_string()
                .contains("列 name 排序规则变化")
        );
    }

    #[test]
    fn integer_migration_checks_signed_and_physical_ranges() {
        let mut before = resource();
        before
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .value_type = ValueType::U32;
        let mut after = before.clone();
        after
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .value_type = ValueType::I64;
        assert!(
            diff::statements(&before, &after)
                .unwrap()
                .join("\n")
                .contains("BIGINT")
        );
        assert!(
            diff::statements(&after, &before)
                .unwrap_err()
                .to_string()
                .contains("人工审阅")
        );
        after
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .value_type = ValueType::I32;
        assert!(diff::statements(&after, &before).is_err());
        before
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .value_type = ValueType::I16;
        after
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .sql_type = Some("TINYINT".into());
        assert!(
            diff::statements(&before, &after).is_err(),
            "Rust 类型变宽不能掩盖物理列变窄"
        );
        let mut unsigned = resource();
        unsigned
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .value_type = ValueType::U32;
        let mut wide = unsigned.clone();
        wide.fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .value_type = ValueType::U64;
        assert!(
            diff::statements(&unsigned, &wide)
                .unwrap()
                .join("\n")
                .contains("BIGINT UNSIGNED")
        );
        assert!(diff::statements(&wide, &unsigned).is_err());
        unsigned
            .fields
            .iter_mut()
            .find(|field| field.name == "status")
            .unwrap()
            .value_type = ValueType::I64;
        assert!(
            diff::statements(&unsigned, &wide).is_err(),
            "有符号旧值不能静默转成无符号列"
        );
        assert!(
            diff::statements(&wide, &unsigned).is_err(),
            "u64 最大值不能存入 i64"
        );
    }
}
