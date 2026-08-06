//! 用于部署任务的独立迁移命令。
//!
//! 生产环境中只有 `ryframe-migrate up` 可以执行 DDL。
//! API 和 Worker 进程改用 `database.migration_mode = "verify"`。

use ryframe_config::{AppConfig, Environment};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let command = std::env::args().nth(1).unwrap_or_else(|| "status".into());
    if !matches!(command.as_str(), "up" | "verify" | "status") {
        return Err("usage: ryframe-migrate <up|verify|status>".into());
    }

    let environment = Environment::from_env()?;
    let config = AppConfig::load_from_env(environment)?;
    ryframe_utils::snowflake::initialize(config.snowflake_worker_id)?;
    let database = ryframe_db::connection::connect_with_sql_logging(
        &config.database.primary,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await?;

    let operation = async {
        match command.as_str() {
            "up" => {
                ryframe_db_migration::up(&database).await?;
                println!("migration completed and schema verified");
            }
            "verify" => {
                ryframe_db_migration::verify(&database).await?;
                println!("migration ledger and schema are current");
            }
            "status" => {
                let status = ryframe_db_migration::status(&database).await?;
                println!(
                    "applied={} expected={} up_to_date={}",
                    status.applied,
                    status.expected,
                    status.is_up_to_date()
                );
            }
            _ => unreachable!("command was validated above"),
        }
        Ok::<(), sea_orm::DbErr>(())
    }
    .await;
    let close_result = database.close().await;
    operation?;
    close_result?;
    Ok(())
}
