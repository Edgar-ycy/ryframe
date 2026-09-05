//! MySQL 真实协议测试共用的精确隔离 fixture；只有显式环境开关才连接外部服务。
use futures_util::FutureExt;
use ryframe_config::{DbConnection, DbTlsMode};
use ryframe_db::connection;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement};
use std::{
    env,
    future::Future,
    process,
    sync::atomic::{AtomicU64, Ordering},
};

const ENABLE_ENV: &str = "RYFRAME_MYSQL_INTEGRATION";

static SCHEMA_SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct MySqlFixture {
    admin: DatabaseConnection,
    database: DatabaseConnection,
    schema: String,
}

impl MySqlFixture {
    async fn create(test_name: &str) -> Result<Self, String> {
        let schema = unique_schema(test_name);
        let admin = connection::connect(&database_config("mysql"))
            .await
            .map_err(|error| format!("连接 MySQL 管理库失败: {error}"))?;
        execute(
            &admin,
            &format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4"),
        )
        .await
        .map_err(|error| format!("创建测试 schema {schema} 失败: {error}"))?;

        let database = match connection::connect(&database_config(&schema)).await {
            Ok(database) => database,
            Err(error) => {
                let _ = execute(&admin, &format!("DROP DATABASE `{schema}`")).await;
                return Err(format!("连接测试 schema {schema} 失败: {error}"));
            }
        };

        Ok(Self {
            admin,
            database,
            schema,
        })
    }

    async fn cleanup(self) -> Result<(), String> {
        let Self {
            admin,
            database,
            schema,
        } = self;
        database
            .close()
            .await
            .map_err(|error| format!("关闭测试 schema {schema} 连接失败: {error}"))?;
        execute(&admin, &format!("DROP DATABASE `{schema}`"))
            .await
            .map_err(|error| format!("精确删除测试 schema {schema} 失败: {error}"))?;
        admin
            .close()
            .await
            .map_err(|error| format!("关闭 MySQL 管理连接失败: {error}"))
    }
}

pub(crate) async fn run_mysql_test<F, Fut>(test_name: &str, test: F)
where
    F: FnOnce(DatabaseConnection) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    if !integration_enabled(ENABLE_ENV) {
        eprintln!("跳过 MySQL 真实协议测试：设置 {ENABLE_ENV}=1 后才会连接外部服务");
        return;
    }

    let fixture = MySqlFixture::create(test_name)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let result = std::panic::AssertUnwindSafe(test(fixture.database.clone()))
        .catch_unwind()
        .await;
    let cleanup = fixture.cleanup().await;
    if let Err(error) = cleanup {
        panic!("{error}");
    }
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("{error}"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

pub(crate) async fn execute<C>(connection: &C, sql: &str) -> Result<(), String>
where
    C: ConnectionTrait,
{
    connection
        .execute_raw(Statement::from_string(DbBackend::MySql, sql))
        .await
        .map(|_| ())
        .map_err(db_error)
}

pub(crate) async fn require_count<C>(connection: &C, sql: &str, expected: i64) -> Result<(), String>
where
    C: ConnectionTrait,
{
    let row = connection
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql))
        .await
        .map_err(db_error)?
        .ok_or_else(|| format!("查询未返回结果: {sql}"))?;
    let actual = row
        .try_get::<i64>("", "value")
        .map_err(|error| format!("读取查询结果失败: {error}"))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "查询结果不匹配，期望 {expected}，实际 {actual}: {sql}"
        ))
    }
}

pub(crate) fn database_config(database: &str) -> DbConnection {
    // 真实协议测试必须走与产品默认配置一致的 AWS-LC TLS 链路。
    // 使用 disabled 会让 caching_sha2_password 的结果依赖 MySQL 进程内认证缓存，
    // 从而在冷服务或并行执行时随机退回到未启用的 RSA 认证后端。
    database_config_with_tls(database, DbTlsMode::Required)
}

pub(crate) fn database_config_with_tls(database: &str, tls_mode: DbTlsMode) -> DbConnection {
    DbConnection {
        host: env_value("RYFRAME_MYSQL_HOST", "127.0.0.1"),
        port: env_parse("RYFRAME_MYSQL_PORT", 3306),
        database: database.to_owned(),
        username: env_value("RYFRAME_MYSQL_USERNAME", "root"),
        password: env_value("RYFRAME_MYSQL_PASSWORD", ""),
        max_connections: 8,
        min_connections: 0,
        acquire_timeout_secs: 5,
        idle_timeout_secs: 30,
        max_lifetime_secs: 60,
        connect_timeout_secs: 5,
        tls_mode,
        tls_ca: None,
        tls_client_cert: None,
        tls_client_key: None,
    }
}

fn unique_schema(test_name: &str) -> String {
    let run = sanitized_fragment(&env_value("RYFRAME_INTEGRATION_RUN_ID", "local"), 16);
    let test = sanitized_fragment(test_name, 16);
    let sequence = SCHEMA_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("ryframe_it_{run}_{test}_{}_{sequence}", process::id())
}

fn sanitized_fragment(value: &str, max_len: usize) -> String {
    value
        .chars()
        .filter_map(|character| {
            let character = character.to_ascii_lowercase();
            character.is_ascii_alphanumeric().then_some(character)
        })
        .take(max_len)
        .collect::<String>()
}

pub(crate) fn integration_enabled(name: &str) -> bool {
    match env::var(name) {
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("{name} 只接受精确值 1，当前值为 {value:?}"),
        Err(env::VarError::NotPresent) => false,
        Err(env::VarError::NotUnicode(_)) => panic!("{name} 必须是有效 UTF-8"),
    }
}

fn env_value(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn env_parse<T>(name: &str, default: T) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|error| panic!("{name} 解析失败: {error}")),
        Err(env::VarError::NotPresent) => default,
        Err(env::VarError::NotUnicode(_)) => panic!("{name} 必须是有效 UTF-8"),
    }
}

pub(crate) fn db_error(error: DbErr) -> String {
    error.to_string()
}
