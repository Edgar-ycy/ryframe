//! RyFrame 数据库重置工具
//!
//! 清空所有业务表后重新执行 sql/ryframe_config.sql 初始化脚本，
//! 并生成正确的 argon2 密码哈希（不再依赖 SQL 文件中的预生成哈希）。
//!
//! 使用方式：
//! ```bash
//! cargo run --bin ryframe-db-reset
//! ```

use std::{fs, path::PathBuf, time::Duration};

use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};

/// 需要删除的全部表（按外键依赖从子到父排列）
const ALL_TABLES: &[&str] = &[
    "sys_role_dept",
    "sys_role_menu",
    "sys_role_permission",
    "sys_user_role",
    "sys_login_info",
    "sys_oper_log",
    "sys_notice",
    "sys_dict_data",
    "sys_dict_type",
    "sys_config",
    "sys_post",
    "sys_menu",
    "sys_permission",
    "sys_role",
    "sys_user",
    "sys_dept",
];

/// 默认用户密码配置
const ADMIN_PASSWORD: &str = "admin123";
const USER_PASSWORD: &str = "123456";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    println!("========================================");
    println!("  RyFrame 数据库重置工具");
    println!("========================================");

    // 1. 加载配置
    let config =
        ryframe_config::AppConfig::load("config").expect("加载配置失败，请确认 config/ 目录存在");
    let db_url = config.database.connections[0].connection_url();
    println!("\n数据库: {}", db_url);

    // 2. 连接数据库
    let conn = connect_db(&db_url).await;
    let backend = conn.get_database_backend();

    // 3. 查找 SQL 文件
    let sql_path = find_sql_file();
    println!("SQL 脚本: {}", sql_path.display());
    if !sql_path.exists() {
        eprintln!("错误: 找不到 sql/ryframe_config.sql");
        eprintln!("请在项目根目录执行: cargo run --bin ryframe-db-reset");
        return;
    }

    // ========== Step 1: 清空所有表 ==========
    println!("\n>>> Step 1: 清空所有表...");

    if matches!(backend, sea_orm::DatabaseBackend::MySql) {
        conn.execute_unprepared("SET FOREIGN_KEY_CHECKS = 0")
            .await
            .expect("关闭外键检查失败");
    }

    let mut dropped = 0usize;
    for table in ALL_TABLES {
        let sql = format!("DROP TABLE IF EXISTS `{}`", table);
        match conn.execute_unprepared(&sql).await {
            Ok(_) => {
                dropped += 1;
                println!("  ✓ DROP {}", table);
            }
            Err(e) => {
                println!("  - {} (跳过: {})", table, e);
            }
        }
    }

    if matches!(backend, sea_orm::DatabaseBackend::MySql) {
        conn.execute_unprepared("SET FOREIGN_KEY_CHECKS = 1")
            .await
            .expect("开启外键检查失败");
    }

    println!("  共处理 {} 张表", dropped);

    // ========== Step 2: 执行 SQL 脚本 ==========
    println!("\n>>> Step 2: 执行 sql/ryframe_config.sql...");

    let sql_content = fs::read_to_string(&sql_path).expect("读取 SQL 文件失败");
    let statements = split_sql_statements(&sql_content);

    let mut executed = 0usize;
    let mut errors = 0usize;

    for stmt in &statements {
        let trimmed = stmt.trim();
        if trimmed.is_empty() {
            continue;
        }

        match conn.execute_unprepared(trimmed).await {
            Ok(_) => executed += 1,
            Err(e) => {
                errors += 1;
                // 只打印前 5 个错误，避免刷屏
                if errors <= 5 {
                    let preview: String = trimmed.chars().take(80).collect();
                    eprintln!("  ✗ SQL 执行失败: {}", preview);
                    eprintln!("    错误: {}", e);
                }
            }
        }
    }

    println!("  成功: {} 条, 失败: {} 条", executed, errors);
    if errors > 0 {
        eprintln!("\n警告: 存在 {} 条 SQL 执行失败，请检查数据库状态", errors);
    }

    // ========== Step 3: 修正密码哈希 ==========
    println!("\n>>> Step 3: 生成正确的 argon2 密码哈希...");

    // 生成哈希
    let admin_hash = ryframe_auth::password::hash(ADMIN_PASSWORD).expect("admin 密码哈希生成失败");
    let user_hash = ryframe_auth::password::hash(USER_PASSWORD).expect("user 密码哈希生成失败");

    println!("  admin ({}) → {}", ADMIN_PASSWORD, admin_hash);
    println!("  user  ({}) → {}", USER_PASSWORD, user_hash);

    // 更新 admin 用户密码
    let update_admin = format!(
        "UPDATE `sys_user` SET `password_hash` = '{}' WHERE `username` = 'admin'",
        escape_sql_string(&admin_hash)
    );
    match conn.execute_unprepared(&update_admin).await {
        Ok(_) => println!("  ✓ UPDATE admin 密码哈希"),
        Err(e) => eprintln!("  ✗ UPDATE admin 失败: {}", e),
    }

    // 更新 user 用户密码
    let update_user = format!(
        "UPDATE `sys_user` SET `password_hash` = '{}' WHERE `username` = 'user'",
        escape_sql_string(&user_hash)
    );
    match conn.execute_unprepared(&update_user).await {
        Ok(_) => println!("  ✓ UPDATE user 密码哈希"),
        Err(e) => eprintln!("  ✗ UPDATE user 失败: {}", e),
    }

    // ========== 完成 ==========
    println!("\n========================================");
    println!("  数据库重置完成！");
    println!("========================================");
    println!("  admin 账号: admin / {}", ADMIN_PASSWORD);
    println!("  user  账号: user  / {}", USER_PASSWORD);
    println!("========================================");
}

/// 连接数据库
async fn connect_db(db_url: &str) -> DatabaseConnection {
    let mut opt = ConnectOptions::new(db_url.to_string());
    opt.max_connections(2)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(15))
        .acquire_timeout(Duration::from_secs(15));

    Database::connect(opt)
        .await
        .expect("数据库连接失败，请确认数据库服务已启动且配置正确")
}

/// 查找项目根目录下的 sql/ryframe_config.sql
fn find_sql_file() -> PathBuf {
    // 尝试多个可能的位置
    let candidates = [
        PathBuf::from("sql/ryframe_config.sql"),       // 项目根目录
        PathBuf::from("../sql/ryframe_config.sql"),    // 从 crates/ryframe/
        PathBuf::from("../../sql/ryframe_config.sql"), // 从 crates/ryframe/src/bin/
    ];

    for path in &candidates {
        if path.exists() {
            return path.clone();
        }
    }

    // 返回默认位置（即使不存在）
    candidates[0].clone()
}

/// 转义 SQL 字符串中的单引号
fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}

/// 简单 SQL 语句分割器
///
/// 以 `;` 为分隔符，忽略空白和纯注释行，跳过空的语句。
fn split_sql_statements(content: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // 跳过空行
        if trimmed.is_empty() {
            continue;
        }

        // 跳过行注释
        if trimmed.starts_with("--") {
            continue;
        }

        current.push_str(line);
        current.push('\n');

        // 遇到分号结束一条语句
        if trimmed.ends_with(';') {
            let stmt = current.trim().to_string();
            // 过滤掉纯 SET / 块注释
            if !stmt.is_empty() && !stmt.starts_with("/*") {
                statements.push(stmt);
            }
            current.clear();
        }
    }

    // 处理最后一条没有分号的语句
    let remaining = current.trim().to_string();
    if !remaining.is_empty() && !remaining.starts_with("/*") {
        statements.push(remaining);
    }

    statements
}
