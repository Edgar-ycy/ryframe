use crate::{AppError, AppResult, ResourceDatabase, RyFrameBusinessModule};
use std::collections::BTreeSet;

/// 当前控制库业务表结构，独立于历史迁移与迁移执行依赖。
#[derive(Clone, Copy, Debug)]
pub struct BusinessControlTableDescriptor {
    pub table: &'static str,
    pub schema_canonical: &'static str,
}

pub(crate) fn validate_catalog(module: &RyFrameBusinessModule) -> AppResult<()> {
    let expected = module
        .resources()
        .iter()
        .filter(|resource| resource.database == ResourceDatabase::Control)
        .map(|resource| resource.table)
        .collect::<BTreeSet<_>>();
    let actual = module
        .control_tables()
        .iter()
        .map(|table| table.table)
        .collect::<BTreeSet<_>>();
    if expected != actual || actual.len() != module.control_tables().len() {
        return Err(AppError::Config(format!(
            "业务模块 {} 的控制库资源与完整结构目录不一致",
            module.name()
        )));
    }
    for table in module.control_tables() {
        if table.schema_canonical.is_empty() {
            return Err(AppError::Config(format!(
                "业务模块 {} 的表 {} 缺少结构描述",
                module.name(),
                table.table
            )));
        }
    }
    Ok(())
}
