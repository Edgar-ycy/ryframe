use std::{
    env,
    future::Future,
    process,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use ryframe_config::{DbConnection, DbTlsMode};
use ryframe_db::{PostRepository, Repository, connection};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, TransactionTrait};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transaction_commit_and_rollback_use_real_mysql() {
    run_mysql_test("transaction", |database| async move {
        execute(
            &database,
            "CREATE TABLE protocol_account (\
                id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,\
                tenant_id VARCHAR(64) NOT NULL,\
                external_key VARCHAR(64) NOT NULL,\
                amount BIGINT NOT NULL,\
                UNIQUE KEY uq_protocol_account (tenant_id, external_key)\
            ) ENGINE=InnoDB",
        )
        .await?;

        let committed = database.begin().await.map_err(db_error)?;
        execute(
            &committed,
            "INSERT INTO protocol_account (tenant_id, external_key, amount) \
             VALUES ('tenant-a', 'committed', 10)",
        )
        .await?;
        committed.commit().await.map_err(db_error)?;

        let rolled_back = database.begin().await.map_err(db_error)?;
        execute(
            &rolled_back,
            "INSERT INTO protocol_account (tenant_id, external_key, amount) \
             VALUES ('tenant-a', 'rolled-back', 20)",
        )
        .await?;
        rolled_back.rollback().await.map_err(db_error)?;

        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM protocol_account WHERE external_key = 'committed'",
            1,
        )
        .await?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM protocol_account WHERE external_key = 'rolled-back'",
            0,
        )
        .await
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_enforce_unique_constraint() {
    run_mysql_test("unique", |database| async move {
        execute(
            &database,
            "CREATE TABLE protocol_account (\
                id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,\
                tenant_id VARCHAR(64) NOT NULL,\
                external_key VARCHAR(64) NOT NULL,\
                amount BIGINT NOT NULL,\
                UNIQUE KEY uq_protocol_account (tenant_id, external_key)\
            ) ENGINE=InnoDB",
        )
        .await?;

        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let first = insert_unique(database.clone(), 11, Arc::clone(&barrier));
        let second = insert_unique(database.clone(), 22, barrier);
        let (first, second) = tokio::join!(first, second);
        let successes = usize::from(first.is_ok()) + usize::from(second.is_ok());
        if successes != 1 {
            return Err(format!(
                "并发唯一键写入必须恰好成功一次，实际结果: first={first:?}, second={second:?}"
            ));
        }
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM protocol_account \
             WHERE tenant_id = 'tenant-a' AND external_key = 'same-key'",
            1,
        )
        .await
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn select_for_update_serializes_concurrent_updates() {
    run_mysql_test("row-lock", |database| async move {
        execute(
            &database,
            "CREATE TABLE protocol_counter (\
                id BIGINT NOT NULL PRIMARY KEY,\
                value BIGINT NOT NULL\
            ) ENGINE=InnoDB",
        )
        .await?;
        execute(
            &database,
            "INSERT INTO protocol_counter (id, value) VALUES (1, 0)",
        )
        .await?;

        let holder = database.begin().await.map_err(db_error)?;
        holder
            .query_one_raw(Statement::from_string(
                DbBackend::MySql,
                "SELECT value FROM protocol_counter WHERE id = 1 FOR UPDATE",
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(|| "未读取到待加锁计数器".to_owned())?;

        let contender_database = database.clone();
        let (ready_sender, ready_receiver) = tokio::sync::oneshot::channel();
        let contender = tokio::spawn(async move {
            let transaction = contender_database.begin().await?;
            let _ = ready_sender.send(());
            transaction
                .execute_raw(Statement::from_string(
                    DbBackend::MySql,
                    "UPDATE protocol_counter SET value = value + 1 WHERE id = 1",
                ))
                .await?;
            transaction.commit().await
        });

        tokio::time::timeout(Duration::from_secs(2), ready_receiver)
            .await
            .map_err(|_| "竞争事务未能及时启动".to_owned())?
            .map_err(|_| "竞争事务在尝试 UPDATE 前意外退出".to_owned())?;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let was_blocked = !contender.is_finished();
        execute(
            &holder,
            "UPDATE protocol_counter SET value = value + 1 WHERE id = 1",
        )
        .await?;
        holder.commit().await.map_err(db_error)?;

        tokio::time::timeout(Duration::from_secs(5), contender)
            .await
            .map_err(|_| "竞争事务在释放行锁后仍未完成".to_owned())?
            .map_err(|error| format!("竞争事务任务失败: {error}"))?
            .map_err(db_error)?;
        if !was_blocked {
            return Err("竞争 UPDATE 未被 SELECT FOR UPDATE 行锁阻塞".to_owned());
        }
        require_count(
            &database,
            "SELECT value FROM protocol_counter WHERE id = 1",
            2,
        )
        .await
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_repository_enforces_tenant_isolation() {
    run_mysql_test("tenant", |database| async move {
        execute(
            &database,
            "CREATE TABLE sys_post (\
                id BIGINT NOT NULL PRIMARY KEY,\
                tenant_id VARCHAR(64) NOT NULL,\
                name VARCHAR(128) NOT NULL,\
                code VARCHAR(64) NOT NULL,\
                sort INT NOT NULL,\
                status VARCHAR(8) NOT NULL,\
                remark VARCHAR(255) NULL,\
                del_flag VARCHAR(8) NOT NULL,\
                created_at DATETIME(6) NOT NULL,\
                updated_at DATETIME(6) NOT NULL,\
                UNIQUE KEY uq_sys_post_tenant_code (tenant_id, code)\
            ) ENGINE=InnoDB",
        )
        .await?;
        execute(
            &database,
            "INSERT INTO sys_post \
             (id, tenant_id, name, code, sort, status, remark, del_flag, created_at, updated_at) \
             VALUES \
             (101, 'tenant-a', '岗位 A', 'shared-code', 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
             (202, 'tenant-b', '岗位 B', 'shared-code', 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
        )
        .await?;

        let repository = PostRepository;
        let tenant_a = repository
            .find_by_code(&database, "tenant-a", "shared-code")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "tenant-a 未读取到自己的岗位".to_owned())?;
        let tenant_b = repository
            .find_by_code(&database, "tenant-b", "shared-code")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "tenant-b 未读取到自己的岗位".to_owned())?;
        if tenant_a.id != 101 || tenant_b.id != 202 {
            return Err(format!(
                "岗位仓储返回了错误租户数据: tenant-a={}, tenant-b={}",
                tenant_a.id, tenant_b.id
            ));
        }

        let cross_tenant = repository
            .find_by_id(&database, "tenant-a", 202)
            .await
            .map_err(|error| error.to_string())?;
        if cross_tenant.is_some() {
            return Err("岗位仓储允许 tenant-a 按主键读取 tenant-b 数据".to_owned());
        }
        Ok(())
    })
    .await;
}

async fn run_mysql_test<F, Fut>(test_name: &str, test: F)
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
    let result = test(fixture.database.clone()).await;
    let cleanup = fixture.cleanup().await;
    if let Err(error) = cleanup {
        panic!("{error}");
    }
    if let Err(error) = result {
        panic!("{error}");
    }
}

async fn insert_unique(
    database: DatabaseConnection,
    amount: i64,
    barrier: Arc<tokio::sync::Barrier>,
) -> Result<(), DbErr> {
    let transaction = database.begin().await?;
    barrier.wait().await;
    let result = transaction
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO protocol_account (tenant_id, external_key, amount) VALUES (?, ?, ?)",
            ["tenant-a".into(), "same-key".into(), amount.into()],
        ))
        .await;
    match result {
        Ok(_) => transaction.commit().await,
        Err(error) => {
            transaction.rollback().await?;
            Err(error)
        }
    }
}

async fn execute<C>(connection: &C, sql: &str) -> Result<(), String>
where
    C: ConnectionTrait,
{
    connection
        .execute_raw(Statement::from_string(DbBackend::MySql, sql))
        .await
        .map(|_| ())
        .map_err(db_error)
}

async fn require_count<C>(connection: &C, sql: &str, expected: i64) -> Result<(), String>
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

fn database_config(database: &str) -> DbConnection {
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
        tls_mode: DbTlsMode::Disabled,
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

fn integration_enabled(name: &str) -> bool {
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

fn db_error(error: DbErr) -> String {
    error.to_string()
}
