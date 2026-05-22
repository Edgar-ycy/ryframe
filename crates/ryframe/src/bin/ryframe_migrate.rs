//! RyFrame 数据库迁移 CLI
//!
//! 使用方式：
//! ```bash
//! # 执行所有待处理的迁移
//! cargo run --bin ryframe-migrate -- up
//!
//! # 回滚最后一次迁移
//! cargo run --bin ryframe-migrate -- down
//!
//! # 查看迁移状态
//! cargo run --bin ryframe-migrate -- status
//!
//! # 重置数据库（回滚所有迁移后重新执行）
//! cargo run --bin ryframe-migrate -- fresh
//! ```

use ryframe_config::AppConfig;
use ryframe_db::migration::Migrator;
use sea_orm_migration::prelude::*;

#[tokio::main]
async fn main() {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // 加载配置
    let config = AppConfig::load("config").expect("加载配置失败");
    let db_url = config.database.primary.connection_url();

    println!("=== RyFrame 数据库迁移工具 ===");
    println!("数据库: {}", db_url);

    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("status");

    let cli = Cli::new(Migrator);

    match command {
        "up" => {
            println!("\n>>> 执行迁移...\n");
            cli.run(command, Some(&db_url)).await;
            println!("\n>>> 迁移完成！");
        }
        "down" => {
            println!("\n>>> 回滚最后一次迁移...\n");
            cli.run(command, Some(&db_url)).await;
            println!("\n>>> 回滚完成！");
        }
        "fresh" => {
            println!("\n>>> 重置数据库（回滚所有 + 重新执行）...\n");
            cli.run(command, Some(&db_url)).await;
            println!("\n>>> 重置完成！");
        }
        _ => {
            println!("\n>>> 迁移状态：\n");
            cli.run("status", Some(&db_url)).await;
        }
    }
}

/// 简单 CLI wrapper
struct Cli<M: MigratorTrait> {
    _migrator: std::marker::PhantomData<M>,
}

impl<M: MigratorTrait> Cli<M> {
    fn new(_migrator: M) -> Self {
        Self {
            _migrator: std::marker::PhantomData,
        }
    }

    async fn run(&self, command: &str, db_url: Option<&str>) {
        let db_url = db_url.expect("数据库 URL 未配置");

        let db = sea_orm::Database::connect(db_url)
            .await
            .expect("数据库连接失败");

        match command {
            "up" => {
                Migrator::up(&db, None).await.expect("迁移执行失败");
            }
            "down" => {
                Migrator::down(&db, None).await.expect("回滚执行失败");
            }
            "fresh" => {
                Migrator::fresh(&db).await.expect("重置执行失败");
            }
            _ => {
                Migrator::status(&db).await.expect("获取状态失败");
            }
        }
    }
}
