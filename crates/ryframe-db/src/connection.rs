use log::LevelFilter;
use ryframe_common::{AppError, AppResult};
use ryframe_config::DbConnection;
use sea_orm::{ConnectOptions, Database, DatabaseConnection, FromQueryResult, Statement};
use std::collections::HashSet;
use std::time::Duration;

/// 根据数据库配置创建连接池
///
/// 支持三种数据库引擎：postgres / mysql / sqlite
pub async fn connect(config: &DbConnection) -> AppResult<DatabaseConnection> {
    let url = config.connection_url();

    let mut opt = ConnectOptions::new(url);
    opt.max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .connect_timeout(Duration::from_secs(10))
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(600));

    // SQLite 需要特殊处理外键和日志级别
    if config.driver == "sqlite" {
        opt.sqlx_logging_level(LevelFilter::Info);
    } else {
        // MySQL/Postgres：借鉴若依，关闭 sqlx 查询日志，避免每一条 SQL 刷屏
        opt.sqlx_logging(false);
    }

    Database::connect(opt)
        .await
        .map_err(|e| AppError::Database(format!("数据库连接失败: {}", e)))
}

/// 健康检查：发送一条简单查询验证连接可用
pub async fn ping(db: &DatabaseConnection) -> AppResult<()> {
    db.ping()
        .await
        .map_err(|e| AppError::Database(format!("数据库健康检查失败: {}", e)))
}

/// 所有必需的业务表（对应 ryframe_config.sql 中的 17 张表）
const REQUIRED_TABLES: &[&str] = &[
    "sys_dept",
    "sys_user",
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
    "sys_job",
    "sys_job_log",
    "user_role",
    "role_permission",
    "role_menu",
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

    // 使用 information_schema 查询当前数据库所有表（兼容 MySQL / PostgreSQL）
    let sql = match backend {
        sea_orm::DatabaseBackend::MySql => {
            "SELECT TABLE_NAME AS table_name FROM information_schema.tables WHERE table_schema = DATABASE()"
        }
        sea_orm::DatabaseBackend::Postgres => {
            "SELECT table_name FROM information_schema.tables WHERE table_schema = current_schema()"
        }
        _ => "SELECT name AS table_name FROM sqlite_master WHERE type = 'table'",
    };

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
