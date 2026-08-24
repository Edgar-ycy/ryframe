use std::collections::BTreeMap;

use ryframe_kernel::AppResult;
use sea_orm::DatabaseConnection;

use crate::resource::{
    AccessSpec, ApiSpec, AuditSpec, DatabaseSpec, ExtensionSpec, FieldSpec, FieldUsageSpec,
    IndexSpec, LabelsSpec, MenuSpec, ResourceError, ResourceIdentitySpec, ResourceSpec, RouteSpec,
    SoftDeleteSpec, StorageSpec, ValidationSpec, ValueType, WidgetSpec,
};

pub use crate::schema::{ColumnInfo, ForeignKeyInfo, IndexInfo, TableInfo};

/// 调用方必须明确提供的业务元数据；数据库结构不会推断这些含义。
#[derive(Debug, Clone)]
pub struct ResourceDraftMetadata {
    pub identity: ResourceIdentitySpec,
    pub storage: StorageSpec,
    pub api: ApiSpec,
    pub access: AccessSpec,
    pub menu: MenuSpec,
    pub route: RouteSpec,
    pub soft_delete: Option<SoftDeleteSpec>,
    pub audit: Option<AuditSpec>,
    pub extensions: ExtensionSpec,
}

/// 由既有表结构得到、仍需人工确认字段用途和控件的资源草案。
#[derive(Debug, Clone)]
pub struct ResourceDraft {
    pub spec: ResourceSpec,
    pub pending_fields: Vec<String>,
    pub pending_notes: Vec<String>,
}

impl ResourceDraft {
    /// 输出确定排序的 TOML 草案；注释明确提示不得跳过人工确认。
    pub fn to_toml(&self) -> Result<String, ResourceError> {
        let body = toml::to_string_pretty(&self.spec).map_err(|error| {
            ResourceError::new(
                format!("无法序列化资源导入草案：{error}"),
                "检查显式元数据是否只包含 TOML 支持的稳定值",
            )
            .with_resource(&self.spec.resource.name)
        })?;
        let mut output = String::from(
            "# 这是既有表导入草案；确认字段用途、校验、控件和业务规则后才能进入生成流程。\n",
        );
        output.push_str(&format!(
            "# 待确认字段：{}\n",
            self.pending_fields.join(", ")
        ));
        for note in &self.pending_notes {
            output.push_str(&format!("# 注意：{note}\n"));
        }
        output.push('\n');
        output.push_str(&body);
        Ok(output)
    }
}

/// 将既有表结构转换为不猜测业务含义的资源草案。
///
/// 字段顺序沿用数据库 ordinal position；主键和索引来自结构元数据。所有字段用途均为
/// `false`、控件均为 `hidden`，由开发者确认后显式修改。函数不连接数据库、不写文件、
/// 不生成产品代码。
pub fn draft_resource_from_table(
    table: &TableInfo,
    metadata: ResourceDraftMetadata,
) -> Result<ResourceDraft, ResourceError> {
    if table.columns.is_empty() {
        return Err(import_error(
            &metadata.identity.name,
            "既有表没有可导入字段",
            "确认读取了正确的 MySQL 表结构",
        ));
    }
    let primary_key = table
        .columns
        .iter()
        .filter(|column| column.is_primary_key)
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    if primary_key.is_empty() {
        return Err(import_error(
            &metadata.identity.name,
            "既有表没有主键，无法形成稳定 CRUD 资源",
            "先为表增加显式主键，再重新导入",
        ));
    }

    let mut fields = Vec::with_capacity(table.columns.len());
    for (position, column) in table.columns.iter().enumerate() {
        let order = u16::try_from((position + 1).saturating_mul(10)).map_err(|_| {
            import_error(
                &metadata.identity.name,
                "字段数量超过资源清单可表达范围",
                "缩小平面资源范围，复杂宽表改用手写用例",
            )
            .with_field(&column.name)
        })?;
        let value_type = import_value_type(&column.data_type).ok_or_else(|| {
            import_error(
                &metadata.identity.name,
                format!("数据库类型 `{}` 无法确定映射", column.data_type),
                "人工选择受支持的 value_type，或将该字段移入强类型扩展",
            )
            .with_field(&column.name)
        })?;
        let database_label = column
            .comment
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&column.name);
        fields.push(FieldSpec {
            name: column.name.clone(),
            value_type,
            wire_type: None,
            order,
            nullable: column.is_nullable,
            default: None,
            usage: pending_usage(),
            validation: ValidationSpec::default(),
            labels: LabelsSpec {
                zh_cn: database_label.to_owned(),
                en: column.name.clone(),
            },
            widget: WidgetSpec::Hidden,
            enum_values: BTreeMap::new(),
        });
    }

    let mut indexes = table
        .indexes
        .iter()
        .filter(|index| !index.name.eq_ignore_ascii_case("primary") && index.columns != primary_key)
        .map(|index| IndexSpec {
            name: index.name.clone(),
            fields: index.columns.clone(),
            unique: index.unique,
        })
        .collect::<Vec<_>>();
    indexes.sort_by(|left, right| left.name.cmp(&right.name));

    let mut pending_notes = table
        .columns
        .iter()
        .filter(|column| column.is_auto_increment)
        .map(|column| {
            format!(
                "字段 {} 是 auto_increment；资源 ID 策略必须人工确认",
                column.name
            )
        })
        .chain(table.foreign_keys.iter().map(|foreign_key| {
            format!(
                "外键 {} 不进入平面资源 DSL，关联行为必须人工确认",
                foreign_key.name
            )
        }))
        .chain(
            table
                .indexes
                .iter()
                .filter(|index| !index.index_type.eq_ignore_ascii_case("btree"))
                .map(|index| {
                    format!(
                        "索引 {} 的类型 {} 不进入资源 DSL",
                        index.name, index.index_type
                    )
                }),
        )
        .collect::<Vec<_>>();
    pending_notes.sort();

    let pending_fields = fields.iter().map(|field| field.name.clone()).collect();
    Ok(ResourceDraft {
        spec: ResourceSpec {
            schema_version: 1,
            resource: metadata.identity,
            storage: metadata.storage,
            database: DatabaseSpec {
                table: table.table_name.clone(),
                primary_key,
                bootstrap_migration: false,
                schema_revision: None,
                indexes,
                soft_delete: metadata.soft_delete,
                audit: metadata.audit,
            },
            fields,
            relations: Vec::new(),
            api: metadata.api,
            access: metadata.access,
            menu: metadata.menu,
            route: metadata.route,
            extensions: metadata.extensions,
        },
        pending_fields,
        pending_notes,
    })
}

/// 只读取既有 MySQL 表结构，供调用方生成并人工确认资源清单草案。
///
/// 本入口不生成产品代码、不写文件，也不推断页面布局或业务事务。
pub async fn inspect_existing_table(
    database: &DatabaseConnection,
    table: &str,
) -> AppResult<TableInfo> {
    crate::schema::fetch_table(database, table).await
}

/// 列出可供显式导入的既有 MySQL 表；结果不触发任何写入。
pub async fn list_existing_tables(database: &DatabaseConnection) -> AppResult<Vec<String>> {
    crate::schema::list_tables(database).await
}

fn pending_usage() -> FieldUsageSpec {
    FieldUsageSpec {
        create: false,
        create_optional: false,
        update: false,
        update_optional: false,
        read: false,
        list: false,
        filter: false,
        sort: false,
    }
}

fn import_value_type(data_type: &str) -> Option<ValueType> {
    match data_type
        .trim()
        .split_once('(')
        .map_or(data_type.trim(), |(base, _)| base)
        .to_ascii_lowercase()
        .as_str()
    {
        "varchar" | "char" | "text" | "longtext" | "mediumtext" | "tinytext" => {
            Some(ValueType::String)
        }
        "tinyint" | "smallint" | "mediumint" | "int" | "integer" => Some(ValueType::I32),
        "bigint" => Some(ValueType::I64),
        "decimal" | "numeric" => Some(ValueType::Decimal),
        "bool" | "boolean" => Some(ValueType::Bool),
        "date" => Some(ValueType::Date),
        "datetime" => Some(ValueType::DateTime),
        "json" => Some(ValueType::Json),
        _ => None,
    }
}

fn import_error(
    resource: &str,
    message: impl Into<String>,
    suggestion: impl Into<String>,
) -> ResourceError {
    ResourceError::new(message, suggestion).with_resource(resource)
}
