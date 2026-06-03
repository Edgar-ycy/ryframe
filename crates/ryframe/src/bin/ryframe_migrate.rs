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
//! # 回滚指定步数
//! cargo run --bin ryframe-migrate -- down 3
//!
//! # 查看迁移状态
//! cargo run --bin ryframe-migrate -- status
//!
//! # 重置数据库（回滚所有迁移后重新执行）
//! cargo run --bin ryframe-migrate -- fresh
//!
//! # 创建新迁移文件
//! cargo run --bin ryframe-migrate -- generate add_user_avatar
//! ```

use std::path::PathBuf;

use ryframe_config::AppConfig;
use ryframe_db::migration::Migrator;
use sea_orm_migration::prelude::*;

#[tokio::main]
async fn main() {
    // 初始化日志（简化输出）
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("status");

    // generate 命令不需要数据库连接
    if command == "generate" {
        if args.len() < 3 {
            eprintln!("❌ 用法: cargo run --bin ryframe-migrate -- generate <migration_name>");
            eprintln!("   示例: cargo run --bin ryframe-migrate -- generate add_user_avatar");
            std::process::exit(1);
        }
        let name = &args[2];
        generate_migration(name);
        return;
    }

    // 加载配置
    let config = match AppConfig::load("config") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("❌ 加载配置失败: {}", e);
            std::process::exit(1);
        }
    };
    let db_url = config.database.connections[0].connection_url();

    print_banner(&db_url, command);

    match command {
        "up" => {
            println!("\n📦 正在执行待处理的迁移...\n");
            run_migration(command, &db_url).await;
            println!("\n✅ 迁移执行完成！");
        }
        "down" => {
            let steps = args.get(2).and_then(|s| s.parse::<u32>().ok());
            let desc = steps
                .map(|n| format!("回滚 {} 步", n))
                .unwrap_or_else(|| "回滚最后一次迁移".into());
            println!("\n⬇️  {}...\n", desc);
            run_migration(command, &db_url).await;
            println!("\n✅ 回滚完成！");
        }
        "fresh" => {
            println!("\n🔄 重置数据库（回滚所有迁移 → 重新执行）...\n");
            run_migration(command, &db_url).await;
            println!("\n✅ 数据库重置完成！");
        }
        _ => {
            println!("\n📊 迁移状态：\n");
            run_migration("status", &db_url).await;
        }
    }
}

fn print_banner(db_url: &str, command: &str) {
    let truncated = if db_url.len() <= 35 {
        db_url.to_string()
    } else {
        format!("{}...", &db_url[..32])
    };
    println!("╔══════════════════════════════════════════╗");
    println!("║     RyFrame 数据库迁移工具               ║");
    println!("╠══════════════════════════════════════════╣");
    println!("║  数据库: {:<31}║", truncated);
    println!("║  命令:   {:<31}║", command);
    println!("╚══════════════════════════════════════════╝");
}

async fn run_migration(command: &str, db_url: &str) {
    let db = match sea_orm::Database::connect(db_url).await {
        Ok(db) => db,
        Err(e) => {
            eprintln!("❌ 数据库连接失败: {}", e);
            eprintln!("   请确认数据库服务已启动，且 config/app.toml 中配置正确");
            std::process::exit(1);
        }
    };

    let result = match command {
        "up" => Migrator::up(&db, None).await,
        "down" => Migrator::down(&db, None).await,
        "fresh" => Migrator::fresh(&db).await,
        _ => Migrator::status(&db).await,
    };

    if let Err(e) = result {
        eprintln!("\n❌ 迁移操作失败: {}", e);
        std::process::exit(1);
    }
}

/// 创建新的迁移文件
///
/// 在 crates/ryframe-db/src/migration/ 目录下创建格式为
/// m{YYYYMMDD}_{HHMMSS}_{name}.rs 的空迁移文件。
fn generate_migration(name: &str) {
    // 仅允许 [a-z][a-z0-9_]* 命名
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        eprintln!("❌ 迁移名称只能包含小写字母、数字和下划线");
        std::process::exit(1);
    }

    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%d_%H%M%S");
    let file_stem = format!("m{}_{}", timestamp, name);
    let file_name = format!("{}.rs", file_stem);

    // 尝试多个可能的迁移目录
    let migration_dir = find_migration_dir();

    let file_path = migration_dir.join(&file_name);
    if file_path.exists() {
        eprintln!("❌ 文件已存在: {}", file_path.display());
        std::process::exit(1);
    }

    let template = r#"use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {{
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {{
        // 在此编写数据库迁移逻辑（建表、加字段、加索引等）
        // 示例：
        // manager.create_table(...).await?;
        Ok(())
    }}

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {{
        // 在此编写回滚逻辑（与 up 中的操作对应，如表删除、字段删除）
        // 示例：
        // manager.drop_table(...).await?;
        Ok(())
    }}
}}
"#;

    match std::fs::write(&file_path, template) {
        Ok(_) => {
            println!("\n✅ 迁移文件已创建: {}", file_path.display());
            println!("\n📝 下一步:");
            println!("   1. 编辑 {} 实现 up/down 逻辑", file_name);
            println!("   2. 在 crates/ryframe-db/src/migration/mod.rs 中添加:");
            println!("      pub mod {};", file_stem);
            println!("   3. 在 Migrator::migrations() 中注册:");
            println!("      Box::new({}::Migration)", file_stem);
            println!("   4. 执行 cargo run --bin ryframe-migrate -- up");
        }
        Err(e) => {
            eprintln!("❌ 创建文件失败: {}", e);
            std::process::exit(1);
        }
    }
}

/// 查找迁移目录
fn find_migration_dir() -> PathBuf {
    let candidates = [
        PathBuf::from("crates/ryframe-db/src/migration"),
        PathBuf::from("../crates/ryframe-db/src/migration"),
        PathBuf::from("../../crates/ryframe-db/src/migration"),
    ];

    for dir in &candidates {
        if dir.exists() {
            return dir.clone();
        }
    }

    eprintln!("❌ 找不到迁移目录。请在项目根目录执行此命令。");
    std::process::exit(1);
}
