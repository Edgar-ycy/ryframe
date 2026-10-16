use ryframe_config::{AppConfig, Environment};
use ryframe_tenant_db::TenantDatabaseTargetRegistry;
use sea_orm::DatabaseConnection;

use super::GenerationPlan;
use crate::import::{TableInfo, inspect_existing_table};

pub(super) async fn execute(plan: &GenerationPlan) -> Result<(), String> {
    crate::business::locate_business_package(&plan.workspace_root, &plan.package)
        .map_err(|error| error.to_string())?;
    let config =
        AppConfig::load_from_env(Environment::from_env().map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let tables = inspect(&config, plan).await?;
    println!(
        "读取 {} 张表；只读来源={}，模型目标={}。",
        tables.len(),
        plan.source,
        plan.database
    );
    super::staging::execute(plan, Some(&tables))
}

async fn inspect(config: &AppConfig, plan: &GenerationPlan) -> Result<Vec<TableInfo>, String> {
    let source = plan.source.as_str();
    let explicit_target = source.strip_prefix("tenant_data.targets.");
    let explicit_control =
        matches!(source, "primary" | "database.primary") || source.starts_with("sources.");
    let bare_control = config
        .database
        .sources
        .iter()
        .any(|candidate| candidate.name == source);
    let bare_target = config
        .tenant_data
        .targets
        .iter()
        .any(|target| target.key == source);
    if !explicit_control && explicit_target.is_none() && bare_control && bare_target {
        return Err(format!(
            "结构来源 {source} 同时匹配控制库 source 与租户 target；使用 sources.{source} 或 tenant_data.targets.{source}；未读取表、未写入文件"
        ));
    }
    if explicit_control || bare_control || (source == "local" && plan.database == "control") {
        let connection = control_connection(config, source).await?;
        let result = inspect_tables(&connection, &plan.tables).await;
        let closed = connection
            .close()
            .await
            .map_err(|error| format!("关闭结构来源连接失败：{error}；未写入源码"));
        return match (result, closed) {
            (Ok(tables), Ok(())) => Ok(tables),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(close)) => Err(format!("{error}；{close}")),
        };
    }
    let key = explicit_target.unwrap_or_else(|| {
        if source == "local" {
            &config.tenant_data.default_target
        } else {
            source
        }
    });
    let registry = TenantDatabaseTargetRegistry::new(
        &config.tenant_data,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .map_err(|error| error.to_string())?;
    let lease = registry.acquire(key).await.map_err(|error| format!("无法取得结构来源 tenant_data.targets.{key}：{error}；检查 target 与密码环境引用；未写入源码"))?;
    inspect_tables(lease.connection(), &plan.tables).await
}

async fn inspect_tables(
    connection: &DatabaseConnection,
    names: &[String],
) -> Result<Vec<TableInfo>, String> {
    let mut tables = Vec::with_capacity(names.len());
    for name in names {
        tables.push(
            inspect_existing_table(connection, name)
                .await
                .map_err(|error| {
                    format!("整批结构读取在表 {name} 失败：{error}；本批未写入源码、未修改数据库")
                })?,
        );
    }
    Ok(tables)
}

async fn control_connection(
    config: &AppConfig,
    source: &str,
) -> Result<DatabaseConnection, String> {
    let connection = if matches!(source, "local" | "primary" | "database.primary") {
        &config.database.primary
    } else {
        let name = source.strip_prefix("sources.").unwrap_or(source);
        &config.database.sources.iter().find(|candidate| candidate.name == name)
            .ok_or_else(|| format!("未找到结构来源 database.sources[{name}]；检查 generator.source 或 --connection；目标类型不决定结构来源"))?.connection
    };
    ryframe_db::connection::connect_with_sql_logging(
        connection,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await
    .map_err(|error| {
        format!("连接结构来源失败：{error}；检查主机、端口、用户权限与 TLS；未写入源码")
    })
}
