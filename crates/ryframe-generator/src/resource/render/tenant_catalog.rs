use std::fmt::Write;

use sha2::{Digest, Sha256};

use super::super::{FieldIr, ResourceIr, StorageKind, ValueType};
use super::schema;

// 与 ryframe-tenant-db 的运行时指纹输入保持一致；schema-import 测试会交叉核验，
// 避免生成器默认 feature 反向依赖数据库 crate。
const TENANT_DATA_FENCE_SCHEMA_CANONICAL: &str = "v4|table=biz_tenant_fence|engine=innodb|charset=utf8mb4|collation=utf8mb4_general_ci|columns=tenant_id:varchar(64):not-null:null-default:no-extra:utf8mb4:utf8mb4_general_ci;target_key:varchar(64):not-null:null-default:no-extra:ascii:ascii_bin;placement_generation:bigint:not-null:null-default:no-extra:none:none;state:varchar(16):not-null:null-default:no-extra:ascii:ascii_bin;switch_token:varchar(64):not-null:null-default:no-extra:ascii:ascii_bin;updated_at:datetime(6):not-null:current_timestamp(6):on update current_timestamp(6):none:none|indexes=PRIMARY:unique:btree:tenant_id;idx_biz_tenant_fence_state:nonunique:btree:state,tenant_id|constraints=PRIMARY:PRIMARY KEY;ck_biz_tenant_fence_generation:CHECK:placement_generation>0;ck_biz_tenant_fence_state:CHECK:statein('active','frozen')";
const TENANT_DATA_TARGET_SLOT_SCHEMA_CANONICAL: &str = "v4|table=biz_tenant_target_slot|engine=innodb|charset=utf8mb4|collation=utf8mb4_general_ci|columns=slot_id:tinyint unsigned:not-null:null-default:no-extra:none:none;tenant_id:varchar(64):nullable:null-default:no-extra:utf8mb4:utf8mb4_general_ci;placement_generation:bigint:nullable:null-default:no-extra:none:none;switch_token:varchar(64):nullable:null-default:no-extra:ascii:ascii_bin;updated_at:datetime(6):not-null:current_timestamp(6):on update current_timestamp(6):none:none|indexes=PRIMARY:unique:btree:slot_id|constraints=PRIMARY:PRIMARY KEY;ck_biz_tenant_target_slot_id:CHECK:slot_id=1;ck_biz_tenant_target_slot_value:CHECK:((tenant_idisnull)and(placement_generationisnull)and(switch_tokenisnull))or((tenant_idisnotnull)and(placement_generation>0)and(switch_tokenisnotnull))";
const RESOURCE_OWNERSHIP_SCHEMA_CANONICAL: &str = "v4|table=ryframe_resource_ownership|engine=innodb|charset=utf8mb4|collation=utf8mb4_general_ci|columns=resource_kind:varchar(32):not-null:null-default:no-extra:ascii:ascii_bin;scope_id:varchar(48):not-null:null-default:no-extra:ascii:ascii_bin;marker:varchar(128):not-null:null-default:no-extra:ascii:ascii_bin;created_at:datetime(6):not-null:current_timestamp(6):no-extra:none:none;updated_at:datetime(6):not-null:current_timestamp(6):on update current_timestamp(6):none:none|indexes=PRIMARY:unique:btree:resource_kind;uq_resource_ownership_marker:unique:btree:marker;uq_resource_ownership_scope:unique:btree:scope_id,resource_kind|constraints=PRIMARY:PRIMARY KEY;uq_resource_ownership_marker:UNIQUE;uq_resource_ownership_scope:UNIQUE";

pub(super) fn render(resources: &[&ResourceIr], header: &str) -> String {
    let resources = resources
        .iter()
        .filter(|resource| resource.storage == StorageKind::TenantData)
        .copied()
        .collect::<Vec<_>>();
    let entries = resources
        .iter()
        .enumerate()
        .map(|(order, resource)| descriptor(resource, order + 1))
        .collect::<Vec<_>>()
        .join("\n");
    let fingerprint = schema_fingerprint(&resources);
    format!(
        "{header}use crate::migration::TenantDataTableDescriptor;\n\npub const GENERATED_TENANT_DATA_TABLES: &[TenantDataTableDescriptor] = &[\n{entries}\n];\n\npub const GENERATED_TENANT_DATA_SCHEMA_FINGERPRINT: &str = {fingerprint:?};\n"
    )
}

fn schema_fingerprint(resources: &[&ResourceIr]) -> String {
    let entries = resources
        .iter()
        .enumerate()
        .map(|(order, resource)| catalog_entry(resource, order + 1))
        .collect::<Vec<_>>();
    let mut canonical = String::from(TENANT_DATA_FENCE_SCHEMA_CANONICAL);
    canonical.push('|');
    canonical.push_str(TENANT_DATA_TARGET_SLOT_SCHEMA_CANONICAL);
    canonical.push('|');
    canonical.push_str(RESOURCE_OWNERSHIP_SCHEMA_CANONICAL);
    canonical.push_str("|catalog=[");
    canonical.push_str(&entries.join(";"));
    canonical.push(']');
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

fn catalog_entry(resource: &ResourceIr, order: usize) -> String {
    let primary_key = resource
        .primary_key
        .iter()
        .map(|field| resource.column(field))
        .collect::<Vec<_>>()
        .join(",");
    let columns = resource
        .fields
        .iter()
        .map(|field| field.column.as_str())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{}:{}:{}:{}:{}::{}",
        resource.table,
        order,
        resource.column(
            resource
                .tenant_field
                .as_deref()
                .expect("租户资源已校验租户字段")
        ),
        primary_key,
        columns,
        canonical_schema(resource),
    )
}

fn descriptor(resource: &ResourceIr, order: usize) -> String {
    let primary_key = resource
        .primary_key
        .iter()
        .map(|field| resource.column(field))
        .collect::<Vec<_>>();
    let columns = resource
        .fields
        .iter()
        .map(|field| field.column.as_str())
        .collect::<Vec<_>>();
    let types = resource
        .fields
        .iter()
        .map(|field| schema::data_type(field.value_type))
        .collect::<Vec<_>>();
    let canonical = canonical_schema(resource);
    format!(
        "    TenantDataTableDescriptor {{\n        table: {table:?},\n        copy_order: {order},\n        tenant_column: {tenant:?},\n        primary_key_cursor_columns: &{primary_key:?},\n        checksum_columns: &{columns:?},\n        column_types: &{types:?},\n        has_generated_columns: false,\n        foreign_key_dependencies: &[],\n        foreign_keys: &[],\n        schema_canonical: {canonical:?},\n    }},",
        table = resource.table,
        tenant = resource.column(
            resource
                .tenant_field
                .as_deref()
                .expect("租户资源已校验租户字段")
        ),
    )
}

fn canonical_schema(resource: &ResourceIr) -> String {
    let mut output = format!(
        "v2|table={:?}|engine={:?}|charset={:?}|collation={:?}|columns=[",
        resource.table, "innodb", "utf8mb4", "utf8mb4_general_ci",
    );
    for field in &resource.fields {
        render_column(&mut output, resource, field);
    }
    let mut indexes = resource
        .indexes
        .iter()
        .map(|index| (index.name.as_str(), index.fields.as_slice(), index.unique))
        .collect::<Vec<_>>();
    indexes.push(("PRIMARY", &resource.primary_key, true));
    // information_schema 的索引与约束按名称（不区分大小写）、列序排列。
    indexes.sort_by_key(|(name, _, _)| name.to_ascii_lowercase());
    output.push_str("]|indexes=[");
    for (name, fields, unique) in &indexes {
        for (position, field) in fields.iter().enumerate() {
            write!(
                output,
                "{:?}:{:?}:{}:{}:{:?}:{:?}:{:?};",
                name,
                resource.column(field),
                position + 1,
                u8::from(!unique),
                "btree",
                None::<i64>,
                "YES"
            )
            .expect("写入 String 不会失败");
        }
    }
    output.push_str("]|constraints=[");
    for (name, _, unique) in indexes {
        if unique {
            let kind = if name == "PRIMARY" {
                "PRIMARY KEY"
            } else {
                "UNIQUE"
            };
            write!(output, "{name:?}:{kind:?}:{:?};", "YES").expect("写入 String 不会失败");
        }
    }
    // 当前资源 DDL 不生成 CHECK 或外键；逻辑关联不能伪装成物理约束。
    output.push_str("]|checks=[]|foreign_keys=[]");
    output
}

fn render_column(output: &mut String, resource: &ResourceIr, field: &FieldIr) {
    let column_type = if field.value_type == ValueType::Bool {
        "tinyint(1)".to_owned()
    } else {
        schema::column_type(field).to_ascii_lowercase()
    };
    let charset = (field.value_type == ValueType::String).then_some("utf8mb4");
    let collation = charset.map(|_| "utf8mb4_general_ci");
    let default = field.default.as_ref().map(|value| match value {
        toml::Value::String(value) => value.clone(),
        toml::Value::Integer(value) => value.to_string(),
        toml::Value::Boolean(value) => u8::from(*value).to_string(),
        _ => unreachable!("默认值已由 IR 校验"),
    });
    write!(
        output,
        "{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?};",
        field.column,
        column_type,
        if field.nullable { "YES" } else { "NO" },
        charset,
        collation,
        column_key(resource, field),
        default,
        "",
        "",
    )
    .expect("写入 String 不会失败");
}

fn column_key(resource: &ResourceIr, field: &FieldIr) -> &'static str {
    if resource.primary_key.contains(&field.name) {
        "PRI"
    } else if resource
        .indexes
        .iter()
        .any(|index| index.unique && index.fields.len() == 1 && index.fields[0] == field.name)
    {
        "UNI"
    } else if resource
        .indexes
        .iter()
        .any(|index| index.fields.first() == Some(&field.name))
    {
        "MUL"
    } else {
        ""
    }
}

#[cfg(all(test, feature = "schema-import"))]
mod tests {
    use std::path::PathBuf;

    use super::{canonical_schema, catalog_entry, schema_fingerprint};

    #[test]
    fn generated_fingerprint_matches_tenant_runtime_contract() {
        let resource = crate::resource::load_resource(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/device.toml"),
        )
        .expect("业务 crate fixture 应有效");
        let resources = [&resource];
        let runtime_entry = ryframe_tenant_db::migration::catalog_entry_canonical(
            &resource.table,
            1,
            resource.column(resource.tenant_field.as_deref().expect("租户字段已校验")),
            &resource
                .primary_key
                .iter()
                .map(|field| resource.column(field))
                .collect::<Vec<_>>(),
            &resource
                .fields
                .iter()
                .map(|field| field.column.as_str())
                .collect::<Vec<_>>(),
            &[] as &[&str],
            &canonical_schema(&resource),
        );
        assert_eq!(catalog_entry(&resource, 1), runtime_entry);
        assert_eq!(
            schema_fingerprint(&resources),
            ryframe_tenant_db::migration::schema_fingerprint_for_catalog(&[runtime_entry])
        );
    }
}
