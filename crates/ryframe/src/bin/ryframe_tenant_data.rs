//! 外部备份登记、完整数据清单与隔离恢复演练维护入口。

use ryframe_config::{AppConfig, Environment};
use ryframe_kernel::AppError;

#[path = "ryframe_tenant_data/args.rs"]
mod args;
#[path = "ryframe_tenant_data/commands.rs"]
mod commands;
#[path = "ryframe_tenant_data/context.rs"]
mod context;
#[path = "ryframe_tenant_data/proof.rs"]
mod proof;
#[path = "ryframe_tenant_data/proof_file.rs"]
mod proof_file;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ryframe::crypto::install_crypto_provider()?;
    let command = args::parse(std::env::args().skip(1)).map_err(AppError::Validation)?;
    if command == args::Command::Help {
        println!("{}", args::USAGE);
        return Ok(());
    }
    let environment = Environment::from_env()?;
    let config = AppConfig::load_from_env(environment)?;
    ryframe_kernel::snowflake::initialize(config.snowflake_worker_id)?;
    ryframe_db::install_id_generator(|| {
        ryframe_kernel::snowflake::try_next_snowflake_id().map_err(AppError::from)
    })?;
    let database = ryframe_db::connection::connect_with_sql_logging(
        &config.database.primary,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await?;
    ryframe_db::migration::verify(&database).await?;
    ryframe_db::resource_ownership::verify_resource_ownership(
        &database,
        config.scope_id.as_str(),
        "control",
    )
    .await?;
    let result = execute(&config, database.clone(), command).await;
    let close_registry = database.close().await;
    result?;
    close_registry?;
    Ok(())
}

async fn execute(
    config: &AppConfig,
    database: sea_orm::DatabaseConnection,
    command: args::Command,
) -> ryframe_kernel::AppResult<()> {
    if let args::Command::TargetInventory { target, output } = &command {
        let verifier = context::database_verifier(config, database)?;
        return commands::target_inventory(config, verifier.as_ref(), target, output).await;
    }
    if command == args::Command::Status {
        let repository = ryframe_db::application_ports::backup::port(
            ryframe_db::ControlDatabaseCluster::single(database),
        );
        return commands::status(config, repository.as_ref()).await;
    }
    let context = context::build(config, database, &command).await?;
    let result = commands::execute(config, &context, command).await;
    let close = context.close().await;
    result?;
    close
}
