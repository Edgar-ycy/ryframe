use ryframe_kernel::{AppError, AppResult};
use ryframe_sdk::BusinessMigration;

/// 导出同一物理控制库的控制面及 shared-control 数据面，不连接数据库。
pub fn shared_control_initialization_sql(
    hashes: &ryframe_db::migration::BootstrapHashes,
    migrations: &[&dyn BusinessMigration],
) -> AppResult<String> {
    let tenant = ryframe_tenant_db::migration::shared_control_initialization()
        .map_err(|error| AppError::Config(error.to_string()))?;
    // 两侧业务一次交给 SDK，业务账本只创建一次，并保留原跨表校验。
    let business = ryframe_sdk::business_initialization_statements(migrations)?;
    let mut statements = tenant.statements;
    statements.extend(business);
    statements.extend(tenant.completion);
    Ok(ryframe_db::migration::mysql_initialization_sql_with_business(hashes, &statements))
}
