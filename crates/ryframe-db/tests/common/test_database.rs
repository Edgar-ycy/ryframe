//! Shared MySQL integration-test database fixture.
//!
//! Every database created here has a fixed, validated prefix. `Drop` releases
//! the pool handle and removes the database on a dedicated runtime, so cleanup
//! also runs while a test is unwinding after a panic. CI additionally tears down
//! the Compose MySQL tmpfs volume with `if: always()` as the hard-abort
//! fallback, where Rust destructors cannot run.

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

/// Owns an isolated MySQL database for the lifetime of one test context.
///
/// The type deliberately does not implement `Clone`: calling `db.clone()`
/// uses `Deref` and clones only the underlying `DatabaseConnection`, while the
/// original fixture remains responsible for cleanup.
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

// SeaORM's query helpers are generic over ConnectionTrait, so Deref alone is
// not sufficient for calls such as `Entity::find().one(&db)`.
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

        // Refuse to execute identifier SQL if memory corruption or a future
        // refactor ever bypasses creation-time validation.
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
                    // The pool was created on the test runtime. Awaiting
                    // `close()` here can wait on connection tasks that cannot
                    // run while the test thread is joining this cleanup
                    // thread. Releasing the handle is sufficient: MySQL can
                    // drop an isolated database while idle sessions still
                    // reference it, and those sessions are closed when their
                    // original runtime resumes.
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
    std::env::var("RYFRAME_TEST_MYSQL_ADMIN_URL").unwrap_or_else(|_| DEFAULT_ADMIN_URL.to_owned())
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
}
