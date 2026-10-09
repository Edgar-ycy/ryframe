use crate::{AppError, AppResult, ResourceDatabase, RyFrameBusinessModule};
use std::collections::BTreeSet;

pub(crate) fn validate_catalog(module: &RyFrameBusinessModule) -> AppResult<()> {
    let expected = module
        .resources()
        .iter()
        .filter(|resource| resource.database == ResourceDatabase::Tenant)
        .map(|resource| resource.table)
        .collect::<BTreeSet<_>>();
    let actual = module
        .tenant_data_tables()
        .iter()
        .map(|table| table.table)
        .collect::<BTreeSet<_>>();
    if expected != actual || actual.len() != module.tenant_data_tables().len() {
        return Err(AppError::Config(format!(
            "业务模块 {} 的租户资源与完整表目录不一致",
            module.name(),
        )));
    }
    for resource in module
        .resources()
        .iter()
        .filter(|resource| resource.database == ResourceDatabase::Tenant)
    {
        let table = module
            .tenant_data_tables()
            .iter()
            .find(|table| table.table == resource.table)
            .expect("已确认完整表集");
        if resource
            .fields
            .iter()
            .any(|field| !table.checksum_columns.contains(&field.column))
            || table.schema_canonical.is_empty()
        {
            return Err(AppError::Config(format!(
                "业务模块 {} 的表 {} 缺少完整列或 schema 描述",
                module.name(),
                resource.table,
            )));
        }
    }
    Ok(())
}
