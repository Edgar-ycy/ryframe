//! 在创建连接与任务前冻结组合根提供的完整租户表目录。

use super::{TENANT_DATA_CATALOG, TenantDataCatalog};
use ryframe_application::ports::tenant_data::TenantDataTableDescriptor;
use std::{collections::BTreeMap, sync::OnceLock};

struct RuntimeCatalog {
    catalog: TenantDataCatalog,
    fingerprint: String,
}

static RUNTIME: OnceLock<RuntimeCatalog> = OnceLock::new();

pub fn compose_catalog(
    business: impl IntoIterator<Item = TenantDataTableDescriptor>,
) -> Result<TenantDataCatalog, String> {
    TENANT_DATA_CATALOG.validate_structure()?;
    if TENANT_DATA_CATALOG.schema_fingerprint()
        != crate::generated::catalog::GENERATED_TENANT_DATA_SCHEMA_FINGERPRINT
    {
        return Err("框架租户目录与生成指纹不一致".into());
    }
    let mut pending = BTreeMap::new();
    for table in TENANT_DATA_CATALOG.tables().iter().copied().chain(business) {
        if pending.insert(table.table, table).is_some() {
            return Err(format!("租户表重复登记：{}", table.table));
        }
    }
    let mut tables = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let ready = pending
            .iter()
            .find_map(|(name, table)| {
                table
                    .foreign_key_dependencies
                    .iter()
                    .all(|dependency| {
                        tables
                            .iter()
                            .any(|done: &TenantDataTableDescriptor| done.table == *dependency)
                    })
                    .then_some(*name)
            })
            .ok_or_else(|| "租户表依赖未知或存在环".to_owned())?;
        let mut table = pending.remove(ready).expect("已确认待处理表");
        table.copy_order =
            u32::try_from(tables.len() + 1).map_err(|_| "租户表数量超出复制顺序范围".to_owned())?;
        tables.push(table);
    }
    let catalog = TenantDataCatalog::composed(tables);
    catalog.validate_structure()?;
    Ok(catalog)
}

pub fn register_business_catalog(
    business: impl IntoIterator<Item = TenantDataTableDescriptor>,
) -> Result<(), String> {
    let catalog = compose_catalog(business)?;
    let value = RuntimeCatalog {
        fingerprint: catalog.schema_fingerprint(),
        catalog,
    };
    if let Err(candidate) = RUNTIME.set(value)
        && RUNTIME
            .get()
            .is_none_or(|current| current.catalog != candidate.catalog)
    {
        return Err("运行时租户目录已冻结，禁止替换模块或表描述".into());
    }
    Ok(())
}

fn runtime() -> &'static RuntimeCatalog {
    RUNTIME.get_or_init(|| RuntimeCatalog {
        catalog: TENANT_DATA_CATALOG.clone(),
        fingerprint: crate::generated::catalog::GENERATED_TENANT_DATA_SCHEMA_FINGERPRINT.into(),
    })
}

pub fn tenant_data_catalog() -> &'static TenantDataCatalog {
    &runtime().catalog
}

pub(super) fn runtime_fingerprint() -> &'static str {
    &runtime().fingerprint
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: TenantDataTableDescriptor = TenantDataTableDescriptor {
        table: "biz_order",
        copy_order: 1,
        tenant_column: "tenant_id",
        primary_key_cursor_columns: &["tenant_id", "id"],
        checksum_columns: &["tenant_id", "id"],
        column_types: &["varchar", "bigint"],
        has_generated_columns: false,
        foreign_key_dependencies: &[],
        foreign_keys: &[],
        schema_canonical: "order-schema",
    };

    #[test]
    fn empty_modules_keep_core_fingerprint() {
        assert_eq!(
            compose_catalog([]).unwrap().schema_fingerprint(),
            TENANT_DATA_CATALOG.schema_fingerprint()
        );
    }

    #[test]
    fn module_order_does_not_change_dependency_order_or_fingerprint() {
        let child = TenantDataTableDescriptor {
            table: "biz_child",
            foreign_key_dependencies: &["biz_order"],
            ..TABLE
        };
        let first = compose_catalog([child, TABLE]).unwrap();
        let second = compose_catalog([TABLE, child]).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.tables()[0].table, "biz_order");
        assert_eq!(first.tables()[1].copy_order, 2);
        assert_ne!(
            first.schema_fingerprint(),
            TENANT_DATA_CATALOG.schema_fingerprint()
        );
    }

    #[test]
    fn duplicate_unknown_dependency_and_incomplete_schema_fail_closed() {
        assert!(compose_catalog([TABLE, TABLE]).is_err());
        assert!(
            compose_catalog([TenantDataTableDescriptor {
                foreign_key_dependencies: &["biz_unknown"],
                ..TABLE
            }])
            .is_err()
        );
        assert!(
            compose_catalog([TenantDataTableDescriptor {
                schema_canonical: "",
                ..TABLE
            }])
            .is_err()
        );
        assert!(
            compose_catalog([TenantDataTableDescriptor {
                checksum_columns: &["id"],
                ..TABLE
            }])
            .is_err()
        );
    }
}
