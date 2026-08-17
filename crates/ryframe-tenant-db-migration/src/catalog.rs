use std::collections::{HashMap, HashSet, VecDeque};

use sha2::{Digest, Sha256};

/// fence 与当前编译期 catalog 的规范化描述。变更 schema/catalog 时必须同步更新。
const TENANT_DATA_FENCE_SCHEMA_CANONICAL: &str = "v4|table=biz_tenant_fence|engine=innodb|charset=utf8mb4|collation=utf8mb4_general_ci|columns=tenant_id:varchar(64):not-null:null-default:no-extra:utf8mb4:utf8mb4_general_ci;target_key:varchar(64):not-null:null-default:no-extra:ascii:ascii_bin;placement_generation:bigint:not-null:null-default:no-extra:none:none;state:varchar(16):not-null:null-default:no-extra:ascii:ascii_bin;switch_token:varchar(64):not-null:null-default:no-extra:ascii:ascii_bin;updated_at:datetime(6):not-null:current_timestamp(6):on update current_timestamp(6):none:none|indexes=PRIMARY:unique:btree:tenant_id;idx_biz_tenant_fence_state:nonunique:btree:state,tenant_id|constraints=PRIMARY:PRIMARY KEY;ck_biz_tenant_fence_generation:CHECK:placement_generation>0;ck_biz_tenant_fence_state:CHECK:statein('active','frozen')";
const TENANT_DATA_TARGET_SLOT_SCHEMA_CANONICAL: &str = "v4|table=biz_tenant_target_slot|engine=innodb|charset=utf8mb4|collation=utf8mb4_general_ci|columns=slot_id:tinyint unsigned:not-null:null-default:no-extra:none:none;tenant_id:varchar(64):nullable:null-default:no-extra:utf8mb4:utf8mb4_general_ci;placement_generation:bigint:nullable:null-default:no-extra:none:none;switch_token:varchar(64):nullable:null-default:no-extra:ascii:ascii_bin;updated_at:datetime(6):not-null:current_timestamp(6):on update current_timestamp(6):none:none|indexes=PRIMARY:unique:btree:slot_id|constraints=PRIMARY:PRIMARY KEY;ck_biz_tenant_target_slot_id:CHECK:slot_id=1;ck_biz_tenant_target_slot_value:CHECK:((tenant_idisnull)and(placement_generationisnull)and(switch_tokenisnull))or((tenant_idisnotnull)and(placement_generation>0)and(switch_tokenisnotnull))";

/// 应用构建所要求的稳定、小写十六进制 SHA-256 schema 指纹。
pub const TENANT_DATA_SCHEMA_FINGERPRINT: &str =
    crate::generated_catalog::GENERATED_TENANT_DATA_SCHEMA_FINGERPRINT;

/// 编译期业务表复制描述。表名和列名不能来自配置或请求。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TenantDataTableDescriptor {
    pub table: &'static str,
    pub copy_order: u32,
    pub tenant_column: &'static str,
    pub primary_key_cursor_columns: &'static [&'static str],
    pub checksum_columns: &'static [&'static str],
    /// 与 checksum/copy 列同序的 information_schema DATA_TYPE。
    pub column_types: &'static [&'static str],
    pub has_generated_columns: bool,
    pub foreign_key_dependencies: &'static [&'static str],
    pub foreign_keys: &'static [TenantDataForeignKeyDescriptor],
    /// 由 generator 从完整 information_schema 规范化生成，用于精确结构校验及指纹。
    pub schema_canonical: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TenantDataForeignKeyDescriptor {
    pub name: &'static str,
    pub columns: &'static [&'static str],
    pub referenced_table: &'static str,
    pub referenced_columns: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub struct TenantDataCatalog {
    tables: &'static [TenantDataTableDescriptor],
}

/// 当前阶段尚未迁入业务表；fence 是路由基础设施，不参与租户数据复制。
pub const TENANT_DATA_CATALOG: TenantDataCatalog = TenantDataCatalog {
    tables: crate::generated_catalog::GENERATED_TENANT_DATA_TABLES,
};

impl TenantDataCatalog {
    /// 构造一个静态、编译期可验证的 catalog。生产使用生成常量；集成测试可注入
    /// 测试专用描述符而无需污染产品 catalog。
    pub const fn new(tables: &'static [TenantDataTableDescriptor]) -> Self {
        Self { tables }
    }

    pub const fn tables(&self) -> &'static [TenantDataTableDescriptor] {
        self.tables
    }

    /// 启动时验证安全命名、租户列、游标/校验列和 FK DAG。
    pub fn validate(&self) -> Result<(), String> {
        self.validate_structure()?;
        let computed = self.schema_fingerprint();
        if computed != TENANT_DATA_SCHEMA_FINGERPRINT {
            return Err("tenant-data schema fingerprint constant is stale".into());
        }
        Ok(())
    }

    /// 由当前 catalog 的完整 canonical schema 计算指纹，测试注入与生产目录共用。
    pub fn schema_fingerprint(&self) -> String {
        let entries = self
            .tables
            .iter()
            .map(|table| {
                catalog_entry_canonical(
                    table.table,
                    table.copy_order,
                    table.tenant_column,
                    table.primary_key_cursor_columns,
                    table.checksum_columns,
                    table.foreign_key_dependencies,
                    table.schema_canonical,
                )
            })
            .collect::<Vec<_>>();
        schema_fingerprint_for_catalog(&entries)
    }

    /// 仅校验目录自身的安全结构，不绑定生产生成指纹。用于测试专用静态 catalog。
    pub fn validate_structure(&self) -> Result<(), String> {
        let mut by_name = HashMap::with_capacity(self.tables.len());
        let mut copy_orders = HashSet::with_capacity(self.tables.len());
        let mut previous_copy_order = None;
        for table in self.tables {
            if !table.table.starts_with("biz_")
                || !safe_identifier(table.table)
                || matches!(table.table, "biz_tenant_fence" | "biz_tenant_target_slot")
            {
                return Err(format!("invalid tenant-data table: {}", table.table));
            }
            if table.tenant_column != "tenant_id" || !safe_identifier(table.tenant_column) {
                return Err(format!("{} must use tenant_id", table.table));
            }
            if table.primary_key_cursor_columns.len() < 2 {
                return Err(format!(
                    "{} primary-key cursor must include tenant_id and a business key",
                    table.table
                ));
            }
            if !table
                .primary_key_cursor_columns
                .iter()
                .all(|column| safe_identifier(column))
                || !table
                    .checksum_columns
                    .iter()
                    .all(|column| safe_identifier(column))
            {
                return Err(format!(
                    "{} contains an unsafe column identifier",
                    table.table
                ));
            }
            if table.primary_key_cursor_columns.first().copied() != Some("tenant_id") {
                return Err(format!(
                    "{} primary-key cursor must start with tenant_id",
                    table.table
                ));
            }
            if !table
                .primary_key_cursor_columns
                .iter()
                .all(|column| table.checksum_columns.contains(column))
            {
                return Err(format!(
                    "{} checksum columns must include the complete primary-key cursor",
                    table.table
                ));
            }
            if !copy_orders.insert(table.copy_order) {
                return Err(format!("duplicate copy_order: {}", table.copy_order));
            }
            if previous_copy_order.is_some_and(|previous| previous >= table.copy_order) {
                return Err("tenant-data catalog slice must be ordered by copy_order".into());
            }
            previous_copy_order = Some(table.copy_order);
            if table.schema_canonical.trim().is_empty() {
                return Err(format!("{} has no canonical schema", table.table));
            }
            if table.has_generated_columns {
                return Err(format!(
                    "{} contains an unsupported generated column",
                    table.table
                ));
            }
            if table.column_types.len() != table.checksum_columns.len() {
                return Err(format!(
                    "{} column_types must align with checksum columns",
                    table.table
                ));
            }
            if table
                .column_types
                .iter()
                .any(|data_type| data_type.eq_ignore_ascii_case("timestamp"))
            {
                return Err(format!(
                    "{} contains unsupported TIMESTAMP; use DATETIME(6)",
                    table.table
                ));
            }
            if !table.checksum_columns.contains(&"tenant_id") {
                return Err(format!("{} checksum omits tenant_id", table.table));
            }
            if by_name.insert(table.table, table).is_some() {
                return Err(format!("duplicate tenant-data table: {}", table.table));
            }
        }

        let mut incoming = self
            .tables
            .iter()
            .map(|table| (table.table, 0usize))
            .collect::<HashMap<_, _>>();
        for table in self.tables {
            let mut dependencies = HashSet::with_capacity(table.foreign_key_dependencies.len());
            for dependency in table.foreign_key_dependencies {
                if !safe_identifier(dependency) {
                    return Err(format!(
                        "{} contains an unsafe dependency identifier",
                        table.table
                    ));
                }
                if !dependencies.insert(*dependency) {
                    return Err(format!(
                        "{} contains duplicate dependency {dependency}",
                        table.table
                    ));
                }
                if !by_name.contains_key(dependency) {
                    return Err(format!(
                        "{} depends on unknown table {dependency}",
                        table.table
                    ));
                }
                if by_name
                    .get(dependency)
                    .is_some_and(|parent| parent.copy_order >= table.copy_order)
                {
                    return Err(format!(
                        "{} dependency {dependency} must have a smaller copy_order",
                        table.table
                    ));
                }
                *incoming.get_mut(table.table).expect("catalog table exists") += 1;
            }
            let mut foreign_key_names = HashSet::with_capacity(table.foreign_keys.len());
            for foreign_key in table.foreign_keys {
                if foreign_key.name.is_empty()
                    || !safe_identifier(foreign_key.name)
                    || !foreign_key_names.insert(foreign_key.name)
                    || foreign_key.columns.is_empty()
                    || foreign_key.columns.len() != foreign_key.referenced_columns.len()
                    || !table
                        .foreign_key_dependencies
                        .contains(&foreign_key.referenced_table)
                    || !foreign_key
                        .columns
                        .iter()
                        .all(|column| safe_identifier(column))
                    || !foreign_key
                        .referenced_columns
                        .iter()
                        .all(|column| safe_identifier(column))
                {
                    return Err(format!(
                        "{} has an invalid foreign-key descriptor",
                        table.table
                    ));
                }
                let local_tenant = foreign_key
                    .columns
                    .iter()
                    .position(|column| *column == "tenant_id");
                let referenced_tenant = foreign_key
                    .referenced_columns
                    .iter()
                    .position(|column| *column == "tenant_id");
                if local_tenant.is_none() || local_tenant != referenced_tenant {
                    return Err(format!(
                        "{} foreign key {} must contain aligned tenant_id columns",
                        table.table, foreign_key.name
                    ));
                }
            }
        }
        let mut ready = incoming
            .iter()
            .filter_map(|(table, degree)| (*degree == 0).then_some(*table))
            .collect::<VecDeque<_>>();
        let mut visited = 0usize;
        while let Some(done) = ready.pop_front() {
            visited += 1;
            for table in self.tables {
                if table.foreign_key_dependencies.contains(&done) {
                    let degree = incoming.get_mut(table.table).expect("catalog table exists");
                    *degree -= 1;
                    if *degree == 0 {
                        ready.push_back(table.table);
                    }
                }
            }
        }
        if visited != self.tables.len() {
            return Err("tenant-data foreign-key dependencies contain a cycle".into());
        }
        Ok(())
    }
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Generator 与运行时共用的单表 catalog 规范化，避免生成和校验各自解释字段。
pub fn catalog_entry_canonical(
    table: &str,
    copy_order: u32,
    tenant_column: &str,
    primary_key_cursor_columns: &[impl AsRef<str>],
    checksum_columns: &[impl AsRef<str>],
    foreign_key_dependencies: &[impl AsRef<str>],
    schema_canonical: &str,
) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        table,
        copy_order,
        tenant_column,
        join_values(primary_key_cursor_columns),
        join_values(checksum_columns),
        join_values(foreign_key_dependencies),
        schema_canonical,
    )
}

fn join_values<T: AsRef<str>>(values: &[T]) -> String {
    values
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(",")
}

/// 由完整基础设施 schema 与有序 catalog 项生成稳定小写 SHA-256。
pub fn schema_fingerprint_for_catalog(entries: &[String]) -> String {
    let mut canonical = String::from(TENANT_DATA_FENCE_SCHEMA_CANONICAL);
    canonical.push('|');
    canonical.push_str(TENANT_DATA_TARGET_SLOT_SCHEMA_CANONICAL);
    canonical.push_str("|catalog=[");
    canonical.push_str(&entries.join(";"));
    canonical.push(']');
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::{TENANT_DATA_CATALOG, TENANT_DATA_SCHEMA_FINGERPRINT};

    #[test]
    fn generated_fingerprint_matches_shared_catalog_computation() {
        assert_eq!(
            TENANT_DATA_CATALOG.schema_fingerprint(),
            TENANT_DATA_SCHEMA_FINGERPRINT
        );
    }
}
