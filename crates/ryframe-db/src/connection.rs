use std::{collections::HashSet, time::Duration};

use log::LevelFilter;
use ryframe_config::{DbConnection, SqlLogLevel};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectOptions, Database, DatabaseConnection, FromQueryResult, Statement};

/// 根据数据库配置创建连接池
///
/// RyFrame v0.5 起仅支持 MySQL 8.0.16 或更高版本。
pub async fn connect(config: &DbConnection) -> AppResult<DatabaseConnection> {
    connect_with_sql_logging(config, SqlLogLevel::Off, 200).await
}

/// 根据数据库配置 + SQL 日志级别创建连接池
pub async fn connect_with_level(
    config: &DbConnection,
    sql_log_level: SqlLogLevel,
) -> AppResult<DatabaseConnection> {
    connect_with_sql_logging(config, sql_log_level, 200).await
}

/// 根据数据库配置和完整 SQL 日志设置创建连接池。
pub async fn connect_with_sql_logging(
    config: &DbConnection,
    sql_log_level: SqlLogLevel,
    slow_threshold_ms: u64,
) -> AppResult<DatabaseConnection> {
    let url = config.connection_url();

    let mut opt = ConnectOptions::new(url);
    opt.max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(Duration::from_secs(config.acquire_timeout_secs))
        .idle_timeout(Duration::from_secs(config.idle_timeout_secs))
        .max_lifetime(Duration::from_secs(config.max_lifetime_secs))
        .connect_timeout(Duration::from_secs(config.connect_timeout_secs));

    // 根据配置控制 SQL 日志输出
    configure_sql_logging(&mut opt, sql_log_level, slow_threshold_ms);

    Database::connect(opt)
        .await
        .map_err(|e| AppError::Database(format!("数据库连接失败: {}", e)))
}

/// 根据 SqlLogLevel 配置 sqlx 日志
fn configure_sql_logging(opt: &mut ConnectOptions, level: SqlLogLevel, slow_threshold_ms: u64) {
    opt.record_stmt_in_spans(false);
    match level {
        SqlLogLevel::Off => {
            opt.sqlx_logging(false);
        }
        SqlLogLevel::Slow => {
            opt.sqlx_logging(true);
            opt.sqlx_logging_level(LevelFilter::Off);
            opt.sqlx_slow_statements_logging_settings(
                LevelFilter::Warn,
                Duration::from_millis(slow_threshold_ms),
            );
        }
        SqlLogLevel::Summary | SqlLogLevel::Full => {
            opt.sqlx_logging(true);
            opt.sqlx_logging_level(LevelFilter::Info);
            opt.sqlx_slow_statements_logging_settings(
                LevelFilter::Warn,
                Duration::from_millis(slow_threshold_ms),
            );
        }
    }
}

/// 健康检查：发送一条简单查询验证连接可用
pub async fn ping(db: &DatabaseConnection) -> AppResult<()> {
    db.ping()
        .await
        .map_err(|e| AppError::Database(format!("数据库健康检查失败: {}", e)))
}

/// 所有必需的业务表（与初始化 SQL 和迁移保持同步）。
const REQUIRED_TABLES: &[&str] = &[
    "sys_tenant",
    "sys_dept",
    "sys_user",
    "password_reset_requests",
    "sys_role",
    "sys_permission",
    "sys_menu",
    "sys_post",
    "sys_config",
    "sys_dict_type",
    "sys_dict_data",
    "sys_notice",
    "sys_oper_log",
    "sys_login_info",
    "sys_file",
    "sys_role_dept",
    "sys_user_role",
    "sys_role_permission",
    "sys_background_job",
    "sys_message",
    "sys_message_audience",
    "sys_message_recipient",
    "sys_tenant_config_bundle",
    "sys_tenant_config_transfer",
    "sys_tenant_config_transfer_item",
    "sys_product_plan",
    "sys_product_plan_version",
    "sys_product_plan_capability",
    "sys_tenant_product_plan",
    "sys_tenant_capability_override",
    "sys_tenant_operation_lease",
    "sys_service_account",
    "sys_service_account_role",
    "sys_service_credential",
    "sys_service_delegation",
    "sys_service_delegation_capability",
    "sys_service_access_audit",
];

#[derive(Debug, FromQueryResult)]
struct TableRow {
    table_name: String,
}

/// 检查所有必需表是否存在
///
/// 返回 `Ok(())` 表示所有表都存在，`Err(missing)` 返回缺失的表名列表。
pub async fn check_tables(db: &DatabaseConnection) -> Result<(), Vec<String>> {
    let backend = db.get_database_backend();
    if backend != sea_orm::DatabaseBackend::MySql {
        return Err(vec!["RyFrame 仅支持 MySQL 数据库连接".into()]);
    }
    let sql = "SELECT TABLE_NAME AS table_name FROM information_schema.tables WHERE table_schema = DATABASE()";

    let results = TableRow::find_by_statement(Statement::from_sql_and_values(backend, sql, []))
        .all(db)
        .await
        .map_err(|e| vec![format!("无法查询表列表: {}", e)])?;

    let existing: HashSet<String> = results.into_iter().map(|r| r.table_name).collect();

    let missing: Vec<String> = REQUIRED_TABLES
        .iter()
        .filter(|t| !existing.contains(**t))
        .map(|t| t.to_string())
        .collect();

    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}
