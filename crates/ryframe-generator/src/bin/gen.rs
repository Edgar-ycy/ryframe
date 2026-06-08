//! RyFrame 代码生成器 — 独立可执行二进制
//!
//! 所有配置均在下方硬编码，无需外部配置文件。
//! 编译仅依赖 ryframe-generator + sea-orm，无需编译 ryframe 主 crate。
//!
//! 使用方式:
//! ```bash
//! cargo run --bin ryframe-gen
//! ```

use std::path::PathBuf;
use std::process;

use ryframe_generator::{GenerateOptions, generate, write_to_disk};
use sea_orm::Database;

// ═══════════════════════════════════════════════════════════
// 配置区域 — 按需修改以下常量
// ═══════════════════════════════════════════════════════════

/// 数据库连接 URL
const DB_URL: &str = "mysql://root:123456@localhost:3306/ryframe_device";

/// workspace 根目录（代码输出的基准路径）
const WORKSPACE_ROOT: &str = "./example";

/// 要生成代码的表名列表
const TABLES: &[&str] = &["t_gongxv", "t_xiangmu"];

/// 表名前缀过滤列表，如 ["t_"] 会将 "t_gongxv" 剥离为 "gongxv" 来命名
const TABLE_PREFIXES: &[&str] = &["t_"];

/// Entity 输出目录（相对于 WORKSPACE_ROOT）
const ENTITY_DIR: &str = "src/entities";

/// Repository 输出目录
const REPOSITORY_DIR: &str = "src/repositories";

/// Service 输出目录
const SERVICE_DIR: &str = "src/service";

/// Handler 输出目录
const HANDLER_DIR: &str = "src/handlers";

/// DTO 输出目录
const DTO_DIR: &str = "src/dto";

/// 是否覆盖已存在的文件
const OVERWRITE: bool = true;

/// 是否在实体中生成数据库注释（字段 + 表级别）
const GENERATE_COMMENTS: bool = true;

// ═══════════════════════════════════════════════════════════

#[tokio::main]
async fn main() {
    if TABLES.is_empty() {
        eprintln!("未配置要生成的表名。请在源代码的 TABLES 常量中添加表名。");
        eprintln!("例如: const TABLES: &[&str] = &[\"sys_user\", \"sys_role\"];");
        process::exit(1);
    }

    let db = match Database::connect(DB_URL).await {
        Ok(db) => db,
        Err(e) => {
            eprintln!("连接数据库失败: {}", e);
            process::exit(1);
        }
    };

    let opts = GenerateOptions {
        tables: TABLES.iter().map(|s| s.to_string()).collect(),
        entity_dir: ENTITY_DIR.into(),
        repository_dir: REPOSITORY_DIR.into(),
        service_dir: SERVICE_DIR.into(),
        handler_dir: HANDLER_DIR.into(),
        dto_dir: DTO_DIR.into(),
        generate_entity: true,
        generate_repository: true,
        generate_service: true,
        generate_handler: true,
        generate_dto: true,
        table_prefixes: TABLE_PREFIXES.iter().map(|s| s.to_string()).collect(),
        generate_comments: GENERATE_COMMENTS,
        overwrite: OVERWRITE,
    };

    println!("数据库: {}", DB_URL);
    println!("生成表: {:?}", opts.tables);
    println!("输出根目录: {}\n", WORKSPACE_ROOT);

    let files = match generate(&db, &opts).await {
        Ok(f) => f,
        Err(e) => {
            eprintln!("代码生成失败: {}", e);
            process::exit(1);
        }
    };

    let root = PathBuf::from(WORKSPACE_ROOT);
    match write_to_disk(&files, &root, opts.overwrite).await {
        Ok(written) => {
            println!("生成完成，写入 {} 个文件:", written.len());
            for p in &written {
                println!("  -> {}", p);
            }
        }
        Err(e) => {
            eprintln!("写入磁盘失败: {}", e);
            process::exit(1);
        }
    }
}
