//! 共享的 MySQL 集成测试数据库夹具。
//!
//! 此处创建的每个数据库都有固定且已校验的前缀。`Drop` 会释放连接池句柄，并在专用
//! runtime 中删除数据库，因此测试在 panic 后展开时也会执行清理。CI 还会通过
//! `if: always()` 拆除 Compose MySQL tmpfs 卷，作为 Rust 析构函数无法运行时的
//! 强制中止回退方案。

use std::{
    ops::Deref,
    sync::atomic::{AtomicU32, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, DbErr, ExecResult, QueryResult,
    Statement, entity::prelude::async_trait,
};

pub const TEST_DATABASE_PREFIX: &str = "ryframe_it_";
const DEFAULT_ADMIN_URL: &str = "mysql://root:ryframe_test_password@127.0.0.1:13306/mysql";
const MAX_PURPOSE_LEN: usize = 16;
const MYSQL_IDENTIFIER_LIMIT: usize = 64;

static DATABASE_SEQUENCE: AtomicU32 = AtomicU32::new(1);

/// 在一个测试上下文的整个生命周期内持有隔离的 MySQL 数据库。
///
/// 此类型刻意不实现 `Clone`：调用 `db.clone()` 会借助 `Deref`，仅克隆底层
/// `DatabaseConnection`，原夹具仍负责清理。
pub struct TestDatabase {
    connection: Option<DatabaseConnection>,
    admin_url: String,
    database_name: String,
}

impl TestDatabase {
    pub async fn create(purpose: &str) -> Self {
        validate_test_database_purpose(purpose)
            .unwrap_or_else(|message| panic!("invalid MySQL test database purpose: {message}"));

        let admin_url = mysql_test_admin_url();
        let database_name = unique_database_name(purpose);
        validate_test_database_name(&database_name)
            .expect("generated MySQL test database name must be safe");

        let admin = Database::connect(&admin_url).await.expect(
            "connect MySQL test service; run `docker compose -f docker-compose.test.yml up -d --wait`",
        );
        admin
            .execute_unprepared(&format!(
                "CREATE DATABASE `{database_name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci"
            ))
            .await
            .expect("create isolated MySQL test database");

        let database_url = database_url(&admin_url, &database_name);
        let connection = match Database::connect(database_url).await {
            Ok(connection) => connection,
            Err(error) => {
                let _ = admin
                    .execute_unprepared(&format!("DROP DATABASE `{database_name}`"))
                    .await;
                panic!("connect isolated MySQL test database: {error}");
            }
        };
        let _ = admin.close().await;

        Self {
            connection: Some(connection),
            admin_url,
            database_name,
        }
    }

    pub fn connection(&self) -> &DatabaseConnection {
        self.connection
            .as_ref()
            .expect("test database connection is available until fixture drop")
    }

    pub fn database_name(&self) -> &str {
        &self.database_name
    }
}

impl Deref for TestDatabase {
    type Target = DatabaseConnection;

    fn deref(&self) -> &Self::Target {
        self.connection()
    }
}

// SeaORM 的查询辅助方法泛型约束为 `ConnectionTrait`，因此仅靠 `Deref` 不足以
// 支持 `Entity::find().one(&db)` 之类的调用。
#[async_trait::async_trait]
impl ConnectionTrait for TestDatabase {
    fn get_database_backend(&self) -> DbBackend {
        self.connection().get_database_backend()
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        self.connection().execute_raw(stmt).await
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        self.connection().execute_unprepared(sql).await
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        self.connection().query_one_raw(stmt).await
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        self.connection().query_all_raw(stmt).await
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let database_name = self.database_name().to_owned();
        let Some(connection) = self.connection.take() else {
            return;
        };
        let admin_url = self.admin_url.clone();

        // 若内存损坏或未来重构绕过创建时校验，则拒绝执行标识符 SQL。
        if let Err(message) = validate_test_database_name(&database_name) {
            eprintln!("refusing unsafe MySQL test database cleanup: {message}");
            return;
        }

        let cleanup = std::thread::Builder::new()
            .name("ryframe-mysql-test-cleanup".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        eprintln!("failed to create runtime for MySQL test database cleanup");
                        return;
                    }
                };

                runtime.block_on(async move {
                    // 连接池创建于测试 runtime。在此等待 `close()` 可能会等待连接
                    // 任务，但测试线程正在等待此清理线程结束，连接任务无法运行。释放
                    // 句柄已足够：即使空闲会话仍引用隔离数据库，MySQL 也可以删除它；
                    // 这些会话会在其原始 runtime 恢复时关闭。
                    drop(connection);
                    let Ok(admin) = Database::connect(&admin_url).await else {
                        eprintln!("failed to connect for MySQL test database cleanup");
                        return;
                    };
                    if admin
                        .execute_unprepared(&format!("DROP DATABASE IF EXISTS `{database_name}`"))
                        .await
                        .is_err()
                    {
                        eprintln!("failed to drop isolated MySQL test database");
                    }
                    let _ = admin.close().await;
                });
            });

        match cleanup {
            Ok(handle) => {
                if handle.join().is_err() {
                    eprintln!("MySQL test database cleanup thread panicked");
                }
            }
            Err(_) => eprintln!("failed to spawn MySQL test database cleanup thread"),
        }
    }
}

pub fn mysql_test_admin_url() -> String {
    mysql_test_admin_url_with(
        std::env::var("RYFRAME_TEST_MYSQL_ADMIN_URL").ok(),
        std::env::var("RYFRAME_TEST_MYSQL_PORT").ok(),
    )
}

fn mysql_test_admin_url_with(admin_url: Option<String>, port: Option<String>) -> String {
    if let Some(admin_url) = admin_url {
        return admin_url;
    }

    let port = port.map_or(13306, |value| {
        value
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .unwrap_or_else(|| panic!("RYFRAME_TEST_MYSQL_PORT 必须是 1 到 65535 之间的端口号"))
    });
    if port == 13306 {
        DEFAULT_ADMIN_URL.to_owned()
    } else {
        format!("mysql://root:ryframe_test_password@127.0.0.1:{port}/mysql")
    }
}

pub fn validate_test_database_purpose(purpose: &str) -> Result<(), String> {
    if purpose.is_empty() || purpose.len() > MAX_PURPOSE_LEN {
        return Err(format!(
            "purpose length must be between 1 and {MAX_PURPOSE_LEN}"
        ));
    }
    if !purpose
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err("purpose may contain only lowercase ASCII letters, digits, and `_`".into());
    }
    Ok(())
}

pub fn validate_test_database_name(database_name: &str) -> Result<(), String> {
    if !database_name.starts_with(TEST_DATABASE_PREFIX) {
        return Err(format!(
            "database name must start with `{TEST_DATABASE_PREFIX}`"
        ));
    }
    if database_name.len() > MYSQL_IDENTIFIER_LIMIT {
        return Err(format!(
            "database name exceeds MySQL's {MYSQL_IDENTIFIER_LIMIT}-byte identifier limit"
        ));
    }
    if !database_name
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(
            "database name may contain only lowercase ASCII letters, digits, and `_`".into(),
        );
    }
    Ok(())
}

fn unique_database_name(purpose: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must be after Unix epoch")
        .as_millis();
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "{TEST_DATABASE_PREFIX}{purpose}_{:x}_{millis:x}_{sequence:x}",
        std::process::id()
    )
}

fn database_url(admin_url: &str, database_name: &str) -> String {
    let (base, query) = admin_url
        .split_once('?')
        .map_or((admin_url, None), |(base, query)| (base, Some(query)));
    let (server, _) = base
        .rsplit_once('/')
        .expect("RYFRAME_TEST_MYSQL_ADMIN_URL must include a database path");
    match query {
        Some(query) if !query.is_empty() => format!("{server}/{database_name}?{query}"),
        _ => format!("{server}/{database_name}?collation=utf8mb4_general_ci"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_database_identifiers_are_strictly_scoped() {
        assert!(super::validate_test_database_purpose("api_2").is_ok());
        assert!(super::validate_test_database_purpose("").is_err());
        assert!(super::validate_test_database_purpose("../mysql").is_err());
        assert!(super::validate_test_database_purpose("UPPER").is_err());
        assert!(super::validate_test_database_name("mysql").is_err());
        assert!(super::validate_test_database_name("ryframe_it_safe_123").is_ok());
        assert!(super::validate_test_database_name("ryframe_it_safe;drop").is_err());
    }

    #[test]
    fn target_url_preserves_admin_query_parameters() {
        assert_eq!(
            super::database_url(
                "mysql://root@localhost/mysql?ssl-mode=disabled",
                "ryframe_it_safe"
            ),
            "mysql://root@localhost/ryframe_it_safe?ssl-mode=disabled"
        );
        assert_eq!(
            super::database_url("mysql://root@localhost/mysql", "ryframe_it_safe"),
            "mysql://root@localhost/ryframe_it_safe?collation=utf8mb4_general_ci"
        );
    }

    #[test]
    fn test_mysql_port_override_builds_an_isolated_admin_url() {
        assert_eq!(
            super::mysql_test_admin_url_with(None, Some("13307".into())),
            "mysql://root:ryframe_test_password@127.0.0.1:13307/mysql"
        );
        assert_eq!(
            super::mysql_test_admin_url_with(
                Some("mysql://custom:secret@db.example/mysql".into()),
                Some("13307".into()),
            ),
            "mysql://custom:secret@db.example/mysql"
        );
    }

    #[test]
    #[should_panic(expected = "RYFRAME_TEST_MYSQL_PORT")]
    fn test_mysql_port_override_rejects_invalid_values() {
        super::mysql_test_admin_url_with(None, Some("invalid".into()));
    }
}
