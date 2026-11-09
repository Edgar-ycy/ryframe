use sha2::{Digest, Sha256};

use super::{IdGeneration, ResourceError, ResourceSpec, is_snake_identifier};

pub(super) fn normalize_schema(
    spec: &ResourceSpec,
    resource: &str,
    source_path: &str,
) -> Result<String, ResourceError> {
    validate_schema_revision(
        resource,
        source_path,
        spec.database.schema_revision.as_deref(),
    )?;
    Ok(database_schema_hash(spec))
}

fn validate_schema_revision(
    resource: &str,
    source_path: &str,
    revision: Option<&str>,
) -> Result<(), ResourceError> {
    let Some(revision) = revision else {
        return Ok(());
    };
    let mut parts = revision.splitn(3, '_');
    let date = parts.next().unwrap_or_default();
    let time = parts.next().unwrap_or_default();
    let label = parts.next().unwrap_or_default();
    let valid = date.len() == 9
        && date.starts_with('m')
        && date[1..].bytes().all(|byte| byte.is_ascii_digit())
        && time.len() == 6
        && time.bytes().all(|byte| byte.is_ascii_digit())
        && is_snake_identifier(label);
    if valid {
        Ok(())
    } else {
        Err(ResourceError::new(
            format!("schema_revision `{revision}` 不是追加迁移名称"),
            "使用 `mYYYYMMDD_HHMMSS_name`，并先通过 cargo xtask data migrate new 创建对应 roll-forward 迁移",
        )
        .with_resource(resource)
        .with_file(source_path))
    }
}

fn database_schema_hash(spec: &ResourceSpec) -> String {
    let fields = spec
        .fields
        .iter()
        .map(|field| {
            let mut contract = serde_json::json!({
                "default": field.default,
                "enum_keys": field.enum_values.keys().collect::<Vec<_>>(),
                "max_length": field.validation.max_length,
                "name": field.name,
                "nullable": field.nullable,
                "value_type": field.value_type,
            });
            if let Some(column) = &field.column {
                contract
                    .as_object_mut()
                    .expect("字段 schema contract 必须是对象")
                    .insert("column".into(), serde_json::json!(column));
            }
            if let Some(sql_type) = &field.sql_type {
                contract
                    .as_object_mut()
                    .expect("字段 schema contract 必须是对象")
                    .insert("sql_type".into(), serde_json::json!(sql_type));
            }
            if let Some(collation) = &field.collation {
                contract
                    .as_object_mut()
                    .expect("字段 schema contract 必须是对象")
                    .insert("collation".into(), serde_json::json!(collation));
            }
            if let Some(comment) = &field.sql_comment {
                contract
                    .as_object_mut()
                    .expect("字段 schema contract 必须是对象")
                    .insert("sql_comment".into(), serde_json::json!(comment));
            }
            contract
        })
        .collect::<Vec<_>>();
    let mut contract = serde_json::json!({
        "fields": fields,
        "indexes": spec.database.indexes,
        "primary_key": spec.database.primary_key,
        "storage": {
            "kind": spec.storage.kind,
            "tenant_field": spec.storage.tenant_field,
        },
        "table": spec.database.table,
    });
    if !spec.database.foreign_keys.is_empty() {
        contract.as_object_mut().expect("schema 是对象").insert(
            "foreign_keys".into(),
            serde_json::json!(spec.database.foreign_keys),
        );
    }
    if spec.database.id_generation != IdGeneration::Snowflake {
        contract.as_object_mut().expect("schema 是对象").insert(
            "id_generation".into(),
            serde_json::json!(spec.database.id_generation),
        );
    }
    if let Some(collation) = &spec.database.collation {
        contract
            .as_object_mut()
            .expect("schema 是对象")
            .insert("collation".into(), serde_json::json!(collation));
    }
    hex::encode(Sha256::digest(
        serde_json::to_vec(&contract).expect("schema contract 只包含可序列化基础类型"),
    ))
}
