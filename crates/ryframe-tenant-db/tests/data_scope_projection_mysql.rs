use std::{env, process};

use ryframe_application::DataScopeProjectionEvent;
use ryframe_config::{DbConnection, DbTlsMode, SqlLogLevel, TenantDataConfig};
use ryframe_db::{ControlDatabaseCluster, connection, migration::BootstrapHashes};
use ryframe_tenant_db::{TenantDataError, TenantDatabaseRouter};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projection_publish_is_versioned_atomic_and_fails_closed() {
    if env::var("RYFRAME_MYSQL_INTEGRATION").as_deref() != Ok("1") {
        eprintln!("跳过数据范围投影测试：设置 RYFRAME_MYSQL_INTEGRATION=1");
        return;
    }
    let fixture = Fixture::create().await;
    let result = run(&fixture.database).await;
    let cleanup = fixture.cleanup().await;
    result.unwrap_or_else(|error| panic!("{error}"));
    cleanup.unwrap_or_else(|error| panic!("{error}"));
}

async fn run(database: &DatabaseConnection) -> Result<(), String> {
    initialize(database).await?;

    let control = ControlDatabaseCluster::single(database.clone());
    let router = TenantDatabaseRouter::new(
        control.clone(),
        &TenantDataConfig::default(),
        SqlLogLevel::Off,
        500,
    )
    .map_err(|error| error.to_string())?;
    let publisher = ryframe_tenant_db::application_ports::data_scope_projection::publisher(
        control,
        router.clone(),
    );
    publisher
        .publish(&event("system", 2))
        .await
        .map_err(|error| error.to_string())?;
    publisher
        .publish(&event("system", 2))
        .await
        .map_err(|error| format!("同版本重复发布应幂等: {error}"))?;

    let (metadata, expected_owners) = assert_initial_projection(database, router.clone()).await?;

    execute(
        database,
        "UPDATE `sys_tenant` SET `data_scope_version` = 3 WHERE `tenant_id` = 'system'",
    )
    .await?;
    assert!(matches!(
        router
            .resolve("system")
            .await
            .map_err(|error| error.to_string())?
            .begin_scoped_read(3)
            .await,
        Err(TenantDataError::DataScopeProjectionStale { .. })
    ));

    publisher
        .publish(&event("system", 3))
        .await
        .map_err(|error| error.to_string())?;
    publisher
        .publish(&event("system", 2))
        .await
        .map_err(|error| format!("旧事件不得覆盖新投影: {error}"))?;
    let latest = load_metadata(database).await?;
    assert_eq!(latest.source_version, 3);
    assert_ne!(latest.build_token, metadata.build_token);
    assert_eq!(
        count(
            database,
            "SELECT COUNT(*) AS `value` FROM `biz_data_scope_owner` WHERE `tenant_id` = 'system'"
        )
        .await?,
        expected_owners
    );
    Ok(())
}

async fn initialize(database: &DatabaseConnection) -> Result<(), String> {
    ryframe_db::migration::up(database, Some(&bootstrap_hashes()))
        .await
        .map_err(|error| format!("初始化控制库失败: {error}"))?;
    ryframe_tenant_db::migration::up(database)
        .await
        .map_err(|error| format!("初始化租户库失败: {error}"))?;

    execute(
        database,
        "INSERT INTO `sys_dept` (`id`, `tenant_id`, `name`, `parent_id`, `ancestors`, `sort`, `status`, `remark`, `del_flag`, `created_at`, `updated_at`) \
         VALUES (10, 'system', '投影部门', NULL, '0', 0, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
    )
    .await?;
    execute(
        database,
        "UPDATE `sys_user` SET `dept_id` = 10 WHERE `tenant_id` = 'system' AND `username` = 'admin'",
    )
    .await?;
    execute(
        database,
        "UPDATE `sys_tenant` SET `data_scope_version` = 2 WHERE `tenant_id` = 'system'",
    )
    .await?;
    Ok(())
}

async fn assert_initial_projection(
    database: &DatabaseConnection,
    router: TenantDatabaseRouter,
) -> Result<(MetadataRow, i64), String> {
    let metadata = load_metadata(database).await?;
    let expected_owners = count(
        database,
        "SELECT COUNT(*) AS `value` FROM `sys_user` \
         WHERE `tenant_id` = 'system' AND `del_flag` = '0'",
    )
    .await?;
    let expected_departments = count(
        database,
        "SELECT COUNT(*) AS `value` FROM `sys_dept` \
         WHERE `tenant_id` = 'system' AND `del_flag` = '0'",
    )
    .await?;
    assert_eq!(metadata.source_version, 2);
    assert!(metadata.ready);
    assert_eq!(metadata.owner_count, expected_owners);
    assert_eq!(metadata.dept_count, expected_departments);
    assert_eq!(metadata.digest.len(), 64);

    let scoped = router
        .resolve("system")
        .await
        .map_err(|error| error.to_string())?
        .begin_scoped_read(2)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(scoped.build_token(), metadata.build_token);
    let owner = OwnerRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT `user_id`, `dept_id` FROM `biz_data_scope_owner` \
         WHERE `tenant_id` = 'system' AND `build_token` = ? AND `user_id` = 1",
        [metadata.build_token.clone().into()],
    ))
    .one(scoped.transaction())
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "投影没有发布 Owner 事实".to_owned())?;
    assert_eq!(owner.user_id, 1);
    assert_eq!(owner.dept_id, Some(10));
    scoped.finish().await.map_err(|error| error.to_string())?;
    Ok((metadata, expected_owners))
}

async fn load_metadata(database: &DatabaseConnection) -> Result<MetadataRow, String> {
    MetadataRow::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT `source_version`, `build_token`, `owner_count`, `dept_count`, `ready`, `digest` \
         FROM `biz_data_scope_projection` WHERE `tenant_id` = 'system'",
    ))
    .one(database)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "投影元数据不存在".to_owned())
}

fn event(tenant_id: &str, source_version: i64) -> DataScopeProjectionEvent {
    DataScopeProjectionEvent {
        tenant_id: tenant_id.into(),
        source_version,
    }
}

fn bootstrap_hashes() -> BootstrapHashes {
    let hash = "$argon2id$v=19$m=65536,t=3,p=4$O8qRRhiIVYjCHpUuwGWTSA$OO+ik8t1+N5a4PSipMbB71W/pfc3roAbq6mdIAgV1bA";
    BootstrapHashes::new(hash.into(), hash.into()).expect("测试引导哈希应有效")
}

async fn execute(database: &DatabaseConnection, sql: &str) -> Result<(), String> {
    database
        .execute_raw(Statement::from_string(DbBackend::MySql, sql))
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn count(database: &DatabaseConnection, sql: &str) -> Result<i64, String> {
    CountRow::find_by_statement(Statement::from_string(DbBackend::MySql, sql))
        .one(database)
        .await
        .map_err(|error| error.to_string())?
        .map(|row| row.value)
        .ok_or_else(|| "计数查询未返回结果".to_owned())
}

#[derive(Debug, FromQueryResult)]
struct MetadataRow {
    source_version: i64,
    build_token: String,
    owner_count: i64,
    dept_count: i64,
    ready: bool,
    digest: String,
}

#[derive(Debug, FromQueryResult)]
struct OwnerRow {
    user_id: i64,
    dept_id: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    value: i64,
}

struct Fixture {
    admin: DatabaseConnection,
    database: DatabaseConnection,
    schema: String,
}

impl Fixture {
    async fn create() -> Self {
        let schema = format!(
            "ryframe_it_scope_{}_{}",
            process::id(),
            chrono::Utc::now().timestamp_micros()
        );
        let admin = connect("mysql").await;
        admin
            .execute_unprepared(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4"))
            .await
            .expect("创建隔离租户库失败");
        let database = connect(&schema).await;
        Self {
            admin,
            database,
            schema,
        }
    }

    async fn cleanup(self) -> Result<(), String> {
        self.database
            .close()
            .await
            .map_err(|error| format!("关闭测试库失败: {error}"))?;
        self.admin
            .execute_unprepared(&format!("DROP DATABASE `{}`", self.schema))
            .await
            .map_err(|error| format!("删除测试库失败: {error}"))?;
        self.admin
            .close()
            .await
            .map_err(|error| format!("关闭 MySQL 管理连接失败: {error}"))
    }
}

async fn connect(database: &str) -> DatabaseConnection {
    connection::connect(&DbConnection {
        host: env::var("RYFRAME_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        port: env::var("RYFRAME_MYSQL_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(3306),
        database: database.into(),
        username: env::var("RYFRAME_MYSQL_USERNAME").unwrap_or_else(|_| "root".into()),
        password: env::var("RYFRAME_MYSQL_PASSWORD").unwrap_or_default(),
        max_connections: 8,
        min_connections: 0,
        acquire_timeout_secs: 5,
        idle_timeout_secs: 30,
        max_lifetime_secs: 60,
        connect_timeout_secs: 5,
        tls_mode: DbTlsMode::Required,
        tls_ca: None,
        tls_client_cert: None,
        tls_client_key: None,
    })
    .await
    .unwrap_or_else(|error| panic!("连接 MySQL {database} 失败: {error}"))
}
