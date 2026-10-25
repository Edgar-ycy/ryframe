//! 同一版本发布职责同时核验 shared-control 与独立 dedicated 目标；失败现场由公共夹具保留。
use ryframe_application::DataScopeProjectionEvent;
use ryframe_config::{
    DbConnection, SqlLogLevel, TenantDataConfig, TenantDatabaseTargetConfig,
    TenantDatabaseTargetKind, TenantDatabaseTargetMode,
};
use ryframe_db::{ControlDatabaseCluster, migration::BootstrapHashes};
use ryframe_tenant_db::{TenantDataError, TenantDatabaseRouter};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
};

#[path = "../../ryframe-db/tests/support/mysql.rs"]
mod mysql;

#[path = "registry/pools_mysql.rs"]
mod registry_pools;

const TARGET: &str = "projection-dedicated";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projection_publish_is_versioned_atomic_and_fails_closed() {
    mysql::run_mysql_test("projection-shared", |database| async move {
        initialize_control(&database).await?;
        ryframe_tenant_db::migration::up(&database)
            .await
            .map_err(message)?;
        let router = router(&database, &TenantDataConfig::default())?;
        exercise(&database, &database, &router).await
    })
    .await;
    mysql::run_mysql_test("projection-control", |control| async move {
        initialize_control(&control).await?;
        mysql::run_mysql_test("projection-dedicated", |target| async move {
            ryframe_tenant_db::migration::up(&target).await.map_err(message)?;
            let schema = database_name(&target).await?;
            assert_ne!(database_name(&control).await?, schema);
            assert_no_control_tables(&target).await?;
            let connection = mysql::MySqlTestConfiguration::load()?.connection(&schema, None)?;
            let router = router(&control, &dedicated_configuration(connection))?;
            let lease = router.targets().acquire(TARGET).await.map_err(message)?;
            let pool = lease.connection().clone();
            drop(lease);
            let result = async {
                mysql::execute(&control, "UPDATE sys_tenant_data_placement SET current_target_key='projection-dedicated', placement_generation=2, switch_token='projection-test', state='active' WHERE tenant_id='system'").await?;
                router.provision_tenant_fence("system", TARGET, 2, "projection-test").await.map_err(message)?;
                exercise(&control, &target, &router).await?;
                assert_no_control_tables(&target).await
            }.await;
            // 注册表中的池不同于 owned 夹具业务池，成功清理前必须显式关闭。
            let close = pool.close().await.map_err(message);
            result.and(close)
        }).await;
        Ok(())
    }).await;
}

fn router(
    control: &DatabaseConnection,
    config: &TenantDataConfig,
) -> Result<TenantDatabaseRouter, String> {
    TenantDatabaseRouter::new(
        ControlDatabaseCluster::single(control.clone()),
        config,
        SqlLogLevel::Off,
        500,
    )
    .map_err(message)
}

fn dedicated_configuration(connection: DbConnection) -> TenantDataConfig {
    let target = TenantDatabaseTargetConfig {
        key: TARGET.into(),
        kind: TenantDatabaseTargetKind::Mysql,
        mode: TenantDatabaseTargetMode::Dedicated,
        host: Some(connection.host),
        port: Some(connection.port),
        database: Some(connection.database),
        username: Some(connection.username),
        password: Some(connection.password),
        tls_mode: Some(connection.tls_mode),
        tls_ca: connection.tls_ca,
        tls_client_cert: connection.tls_client_cert,
        tls_client_key: connection.tls_client_key,
        max_connections: Some(2),
        display_name: Some("投影独立目标".into()),
        ..TenantDatabaseTargetConfig::shared_control()
    };
    TenantDataConfig {
        default_target: TARGET.into(),
        max_open_targets: 1,
        max_total_connections: 2,
        targets: vec![target],
        ..TenantDataConfig::default()
    }
}

async fn exercise(
    control: &DatabaseConnection,
    target: &DatabaseConnection,
    router: &TenantDatabaseRouter,
) -> Result<(), String> {
    let publisher = ryframe_tenant_db::application_ports::data_scope_projection::publisher(
        ControlDatabaseCluster::single(control.clone()),
        router.clone(),
    );
    let transaction = target.begin().await.map_err(message)?;
    require_write_projection_unavailable(&transaction, 2).await?;
    transaction.rollback().await.map_err(message)?;
    publisher.publish(&event(2)).await.map_err(message)?;
    assert_projection(control, target, router, 2, 10).await?;
    assert_write_projection_guard(target).await?;
    let published_token = load_metadata(target).await?.build_token;
    let published = projection_image(target).await?;
    publisher.publish(&event(2)).await.map_err(message)?;
    assert_eq!(projection_image(target).await?, published);
    mysql::execute(control, "INSERT INTO sys_dept(id,tenant_id,name,parent_id,ancestors,sort,status,remark,del_flag,created_at,updated_at) VALUES(11,'system','后继部门',10,'0,10',0,'1',NULL,'0',UTC_TIMESTAMP(6),UTC_TIMESTAMP(6))").await?;
    mysql::execute(
        control,
        "UPDATE sys_user SET dept_id=11 WHERE tenant_id='system' AND username='admin'",
    )
    .await?;
    mysql::execute(
        control,
        "UPDATE sys_tenant SET data_scope_version=3 WHERE tenant_id='system'",
    )
    .await?;
    require_stale(router).await?;
    // owner批次先写入；在部门写入处失败，完整前后像须捕获所有build的残留。
    target.execute_unprepared("CREATE TRIGGER projection_test_reject_dept BEFORE INSERT ON biz_data_scope_dept FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='projection-test-reject-dept'")
        .await.map_err(|error| format!("创建投影故障触发器失败，尚未执行故障发布：{error}"))?;
    let failure = publisher.publish(&event(3)).await.unwrap_err();
    assert!(
        matches!(failure, ryframe_kernel::AppError::Database(ref detail)
        if detail.contains("projection-test-reject-dept"))
    );
    assert_eq!(projection_image(target).await?, published);
    require_stale(router).await?;
    target
        .execute_unprepared("DROP TRIGGER projection_test_reject_dept")
        .await
        .map_err(|error| format!("移除投影故障触发器失败，尚未执行恢复发布：{error}"))?;
    publisher.publish(&event(3)).await.map_err(message)?;
    assert_projection(control, target, router, 3, 11).await?;
    let latest = projection_image(target).await?;
    publisher.publish(&event(2)).await.map_err(message)?;
    assert_eq!(projection_image(target).await?, latest);
    assert_ne!(load_metadata(target).await?.build_token, published_token);
    Ok(())
}

async fn require_stale(router: &TenantDatabaseRouter) -> Result<(), String> {
    assert!(matches!(
        router
            .resolve("system")
            .await
            .map_err(message)?
            .begin_scoped_read(3)
            .await,
        Err(TenantDataError::DataScopeProjectionStale { .. })
    ));
    Ok(())
}

async fn require_write_projection_unavailable<C: ConnectionTrait>(
    target: &C,
    expected: i64,
) -> Result<(), String> {
    let error = ryframe_db::data_scope::lock_projected_build(target, "system", expected)
        .await
        .expect_err("缺失、落后或未就绪投影必须拒绝写入");
    let ryframe_kernel::AppError::TenantDataTargetUnavailable(detail, retry_after) = error else {
        return Err("投影写入栅栏必须报告业务数据暂不可用，不能要求刷新产品会话".into());
    };
    assert_eq!(retry_after, 5);
    assert!(detail.contains("system"));
    assert!(!detail.contains("刷新会话"));
    Ok(())
}

async fn assert_write_projection_guard(target: &DatabaseConnection) -> Result<(), String> {
    let before = projection_image(target).await?;
    let expected_token = load_metadata(target).await?.build_token;
    let transaction = target.begin().await.map_err(message)?;
    let token = ryframe_db::data_scope::lock_projected_build(&transaction, "system", 2)
        .await
        .map_err(message)?;
    assert_eq!(token, expected_token);
    require_write_projection_unavailable(&transaction, 3).await?;
    transaction.rollback().await.map_err(message)?;
    for sql in [
        "UPDATE biz_data_scope_projection SET ready=0 WHERE tenant_id='system'",
        "UPDATE biz_data_scope_projection SET build_token='invalid' WHERE tenant_id='system'",
    ] {
        let transaction = target.begin().await.map_err(message)?;
        transaction.execute_unprepared(sql).await.map_err(message)?;
        require_write_projection_unavailable(&transaction, 2).await?;
        transaction.rollback().await.map_err(message)?;
        assert_eq!(projection_image(target).await?, before);
    }
    Ok(())
}

async fn initialize_control(database: &DatabaseConnection) -> Result<(), String> {
    ryframe_db::migration::up(database, Some(&bootstrap_hashes()))
        .await
        .map_err(message)?;
    mysql::execute(database, "INSERT INTO sys_dept(id,tenant_id,name,parent_id,ancestors,sort,status,remark,del_flag,created_at,updated_at) VALUES(10,'system','投影部门',NULL,'0',0,'1',NULL,'0',UTC_TIMESTAMP(6),UTC_TIMESTAMP(6))").await?;
    mysql::execute(
        database,
        "UPDATE sys_user SET dept_id=10 WHERE tenant_id='system' AND username='admin'",
    )
    .await?;
    mysql::execute(
        database,
        "UPDATE sys_tenant SET data_scope_version=2 WHERE tenant_id='system'",
    )
    .await
}

async fn assert_projection(
    control: &DatabaseConnection,
    target: &DatabaseConnection,
    router: &TenantDatabaseRouter,
    version: i64,
    department: i64,
) -> Result<(), String> {
    let metadata = load_metadata(target).await?;
    assert_eq!(metadata.source_version, version);
    assert!(metadata.ready);
    assert_eq!(metadata.digest.len(), 64);
    let owners = rows(control, "SELECT CAST(JSON_ARRAY(id,dept_id) AS CHAR) AS item FROM sys_user WHERE tenant_id='system' AND del_flag='0' ORDER BY id").await?;
    let departments = rows(control, "SELECT CAST(JSON_ARRAY(id,ancestors) AS CHAR) AS item FROM sys_dept WHERE tenant_id='system' AND del_flag='0' ORDER BY id").await?;
    assert_eq!(
        metadata.owner_count,
        i64::try_from(owners.len()).map_err(message)?
    );
    assert_eq!(
        metadata.dept_count,
        i64::try_from(departments.len()).map_err(message)?
    );
    assert_eq!(rows(target, "SELECT CAST(JSON_ARRAY(o.user_id,o.dept_id) AS CHAR) AS item FROM biz_data_scope_owner o JOIN biz_data_scope_projection p ON o.tenant_id=p.tenant_id AND o.build_token=p.build_token WHERE o.tenant_id='system' ORDER BY o.user_id").await?, owners);
    assert_eq!(rows(target, "SELECT CAST(JSON_ARRAY(d.dept_id,d.ancestors) AS CHAR) AS item FROM biz_data_scope_dept d JOIN biz_data_scope_projection p ON d.tenant_id=p.tenant_id AND d.build_token=p.build_token WHERE d.tenant_id='system' ORDER BY d.dept_id").await?, departments);
    let scoped = router
        .resolve("system")
        .await
        .map_err(message)?
        .begin_scoped_read(version)
        .await
        .map_err(message)?;
    assert_eq!(
        database_name(scoped.transaction()).await?,
        database_name(target).await?
    );
    assert_eq!(scoped.build_token(), metadata.build_token);
    let owner = OwnerRow::find_by_statement(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT user_id,dept_id FROM biz_data_scope_owner WHERE tenant_id='system' AND build_token=? AND user_id=1",
        [metadata.build_token.into()],
    )).one(scoped.transaction()).await.map_err(message)?.ok_or("投影没有发布Owner事实")?;
    assert_eq!(owner.user_id, 1);
    assert_eq!(owner.dept_id, Some(department));
    scoped.finish().await.map_err(message)
}

async fn projection_image(database: &DatabaseConnection) -> Result<Vec<Vec<String>>, String> {
    let mut result = Vec::new();
    for sql in [
        "SELECT CAST(JSON_ARRAY(tenant_id,source_version,build_token,placement_generation,switch_token,digest,owner_count,dept_count,ready,published_at,updated_at) AS CHAR) AS item FROM biz_data_scope_projection ORDER BY tenant_id",
        "SELECT CAST(JSON_ARRAY(tenant_id,build_token,user_id,dept_id) AS CHAR) AS item FROM biz_data_scope_owner ORDER BY tenant_id,build_token,user_id",
        "SELECT CAST(JSON_ARRAY(tenant_id,build_token,dept_id,ancestors) AS CHAR) AS item FROM biz_data_scope_dept ORDER BY tenant_id,build_token,dept_id",
    ] {
        result.push(rows(database, sql).await?);
    }
    Ok(result)
}

async fn rows<C: ConnectionTrait>(database: &C, sql: &str) -> Result<Vec<String>, String> {
    database
        .query_all_raw(Statement::from_string(DbBackend::MySql, sql))
        .await
        .map_err(message)?
        .into_iter()
        .map(|row| row.try_get("", "item").map_err(message))
        .collect()
}

async fn database_name<C: ConnectionTrait>(database: &C) -> Result<String, String> {
    database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT DATABASE() AS name",
        ))
        .await
        .map_err(message)?
        .ok_or("缺少实际数据库名")?
        .try_get("", "name")
        .map_err(message)
}

async fn assert_no_control_tables(database: &DatabaseConnection) -> Result<(), String> {
    mysql::require_count(database, "SELECT CAST(COUNT(*) AS SIGNED) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name IN ('sys_user','sys_dept')", 0).await
}

async fn load_metadata(database: &DatabaseConnection) -> Result<MetadataRow, String> {
    MetadataRow::find_by_statement(Statement::from_string(DbBackend::MySql,
        "SELECT source_version,build_token,owner_count,dept_count,ready,digest FROM biz_data_scope_projection WHERE tenant_id='system'",
    )).one(database).await.map_err(message)?.ok_or_else(|| "投影元数据不存在".into())
}

fn event(source_version: i64) -> DataScopeProjectionEvent {
    DataScopeProjectionEvent {
        tenant_id: "system".into(),
        source_version,
    }
}

fn bootstrap_hashes() -> BootstrapHashes {
    let hash = "$argon2id$v=19$m=65536,t=3,p=4$O8qRRhiIVYjCHpUuwGWTSA$OO+ik8t1+N5a4PSipMbB71W/pfc3roAbq6mdIAgV1bA";
    BootstrapHashes::new(hash.into(), hash.into()).expect("测试引导哈希应有效")
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
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
