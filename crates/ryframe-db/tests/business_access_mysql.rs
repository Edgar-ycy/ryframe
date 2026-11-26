//! 独立测试进程安装业务目录，避免影响框架固定快照与其他真实协议测试。
use ryframe_application::{
    ports::system::{
        LoginInfoFilter, LoginInfoPersistencePort, LoginInfoRecord, OperLogFilter,
        OperLogPersistencePort, OperLogRecord,
    },
    system::operations::{
        ExportSelection, LoginLogExportFilter, OperLogExportFilter, parse_log_time_range,
    },
};
use ryframe_db::{
    ControlDatabaseCluster,
    application_ports::{export, system},
    migration,
};
use ryframe_kernel::{
    ActorContext, DataScope, ExportCursorWindow, PaginationPolicy, ValidatedPageQuery,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use time::{UtcOffset, macros::datetime};

#[path = "support/mysql.rs"]
mod mysql;
use mysql::{db_error, execute, require_count, run_mysql_test};

const ORDER: &str = include_str!("../../ryframe-sdk/tests/fixtures/order_access.toml");
const LABEL: &str = "O'Reilly\\路径";

fn hashes() -> migration::BootstrapHashes {
    let hash = "$argon2id$v=19$m=65536,t=3,p=4$O8qRRhiIVYjCHpUuwGWTSA$OO+ik8t1+N5a4PSipMbB71W/pfc3roAbq6mdIAgV1bA";
    migration::BootstrapHashes::new(hash.into(), hash.into()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn business_access_initialization_is_equivalent_lossless_and_verified_without_writes() {
    let source = ORDER.replace("订单", "O'Reilly\\\\路径");
    let catalog =
        migration::register_business_access_catalog([("order", source.as_str())]).unwrap();
    assert_eq!(catalog.permission_names().len(), 4);
    migration::register_business_access_catalog([("order", source.as_str())]).unwrap();
    assert!(migration::register_business_access_catalog([("order", ORDER)]).is_err());
    let initialization = migration::mysql_initialization_sql(&hashes());
    run_mysql_test("biz-access-migrate", |migrated| async move {
        migration::up(&migrated, Some(&hashes())).await.map_err(db_error)?;
        migration::verify(&migrated).await.map_err(db_error)?;
        let expected = inventory(&migrated).await?;
        assert_lossless(&migrated).await?;
        for mode in ["STRICT_TRANS_TABLES", "STRICT_TRANS_TABLES,NO_BACKSLASH_ESCAPES"] {
            let script = format!("SET SESSION sql_mode='{mode}';\n{initialization}");
            let expected = expected.clone();
            run_mysql_test("biz-access-import", move |imported| async move {
                imported.execute_unprepared(&script).await.map_err(db_error)?;
                migration::verify(&imported).await.map_err(db_error)?;
                assert_eq!(inventory(&imported).await?, expected);
                assert_lossless(&imported).await?;
                verify_drift_is_read_only(&imported).await
            }).await;
        }
        execute(&migrated, "UPDATE sys_menu SET name='自定义名称',visible=0,status='0' WHERE route_key='order.order'").await?;
        migration::verify(&migrated).await.map_err(db_error)?;
        migration::up(&migrated, None).await.map_err(db_error)?;
        require_count(&migrated, "SELECT COUNT(*) AS value FROM sys_menu WHERE route_key='order.order' AND name='自定义名称' AND visible=0", 1).await?;
        verify_missing_is_read_only(&migrated).await
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn log_timestamps_round_trip_in_mysql_and_keep_inclusive_bounds() {
    let source = ORDER.replace("订单", "O'Reilly\\\\路径");
    migration::register_business_access_catalog([("order", source.as_str())]).unwrap();

    run_mysql_test("log-utc", |database| async move {
        migration::up(&database, Some(&hashes()))
            .await
            .map_err(db_error)?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA=DATABASE() AND DATA_TYPE='datetime' AND DATETIME_PRECISION=0 \
             AND ((TABLE_NAME='sys_oper_log' AND COLUMN_NAME='oper_time') \
             OR (TABLE_NAME='sys_login_info' AND COLUMN_NAME='login_time'))",
            2,
        )
        .await?;
        insert_legacy_log_rows(&database).await?;
        verify_log_time_round_trip(&database).await
    })
    .await;
}

async fn insert_legacy_log_rows(database: &DatabaseConnection) -> Result<(), String> {
    execute(
        database,
        "INSERT INTO sys_oper_log \
         (id,tenant_id,title,business_type,method,request_method,oper_name,oper_url,oper_ip,oper_time) VALUES \
         (40,'system','fixture','UPDATE','fixture','PUT','tester','/fixture','127.0.0.1','2026-08-20 23:59:57'), \
         (41,'system','fixture','UPDATE','fixture','PUT','tester','/fixture','127.0.0.1','2026-08-20 23:59:58'), \
         (43,'system','fixture','UPDATE','fixture','PUT','tester','/fixture','127.0.0.1','2026-08-21 00:00:01')",
    )
    .await?;
    execute(
        database,
        "INSERT INTO sys_login_info (id,tenant_id,user_name,ipaddr,login_time) VALUES \
         (40,'system','tester','127.0.0.1','2026-08-20 23:59:57'), \
         (41,'system','tester','127.0.0.1','2026-08-20 23:59:58'), \
         (43,'system','tester','127.0.0.1','2026-08-21 00:00:01')",
    )
    .await?;
    Ok(())
}

async fn verify_log_time_round_trip(database: &DatabaseConnection) -> Result<(), String> {
    let cluster = ControlDatabaseCluster::single(database.clone());
    let operations = system::oper_log(cluster.clone());
    let logins = system::login_info(cluster.clone());
    // 小数低于半秒，保留 DATETIME(0) 在舍入和截断模式下的共同预期。
    let recorded_at = datetime!(2026-08-21 08:00:00.123456 +08:00);
    operations
        .insert(
            "system",
            OperLogRecord {
                id: 42,
                event_id: None,
                request_id: None,
                title: "fixture".into(),
                business_type: "UPDATE".into(),
                method: "fixture".into(),
                request_method: "PUT".into(),
                oper_name: "tester".into(),
                oper_url: "/fixture".into(),
                oper_ip: "127.0.0.1".into(),
                oper_location: None,
                oper_param: None,
                json_result: None,
                status: "1".into(),
                error_message: None,
                oper_time: recorded_at,
                cost_time: 1,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    logins
        .insert(
            "system",
            LoginInfoRecord {
                id: 42,
                user_name: "tester".into(),
                ipaddr: "127.0.0.1".into(),
                login_location: None,
                browser: None,
                os: None,
                status: "1".into(),
                message: None,
                login_time: recorded_at,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    require_count(
        database,
        "SELECT COUNT(*) AS value FROM sys_oper_log \
         WHERE id=42 AND tenant_id='system' AND oper_time='2026-08-21 00:00:00'",
        1,
    )
    .await?;
    require_count(
        database,
        "SELECT COUNT(*) AS value FROM sys_login_info \
         WHERE id=42 AND tenant_id='system' AND login_time='2026-08-21 00:00:00'",
        1,
    )
    .await?;
    verify_log_time_ranges(&cluster, operations.as_ref(), logins.as_ref()).await
}

async fn verify_log_time_ranges(
    cluster: &ControlDatabaseCluster,
    operations: &dyn OperLogPersistencePort,
    logins: &dyn LoginInfoPersistencePort,
) -> Result<(), String> {
    const BEGIN_TIME: &str = "2026-08-21T07:59:58+08:00";
    const END_TIME: &str = "2026-08-20T18:30:00-05:30";
    let page = ValidatedPageQuery::new(1, 10, PaginationPolicy::new(10, 100))
        .map_err(|error| error.to_string())?;
    let actor = ActorContext {
        user_id: 1,
        tenant_id: "system".into(),
        username: "tester".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: false,
        data_scope_version: 1,
        is_super_admin: true,
    };
    let scope = actor.data_scope_context();
    let (begin_time, end_time) = parse_log_time_range(Some(BEGIN_TIME), Some(END_TIME))
        .map_err(|error| error.to_string())?;
    let operation_filter = OperLogFilter {
        oper_name: None,
        status: None,
        begin_time,
        end_time,
    };
    let login_filter = LoginInfoFilter {
        user_name: None,
        status: None,
        begin_time,
        end_time,
    };
    let operations_page = operations
        .find_by_page("system", page, operation_filter, &scope)
        .await
        .map_err(|error| error.to_string())?;
    let logins_page = logins
        .find_by_page("system", page, login_filter, &scope)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!((operations_page.total, logins_page.total), (2, 2));

    let operation_time =
        |record: &OperLogRecord| (record.id, record.oper_time, record.oper_time.offset());
    let login_time =
        |record: &LoginInfoRecord| (record.id, record.login_time, record.login_time.offset());
    let first = (41, datetime!(2026-08-20 23:59:58 UTC), UtcOffset::UTC);
    let last = (42, datetime!(2026-08-21 00:00:00 UTC), UtcOffset::UTC);
    assert_eq!(
        operations_page
            .records
            .iter()
            .map(operation_time)
            .collect::<Vec<_>>(),
        [last, first],
    );
    assert_eq!(
        logins_page
            .records
            .iter()
            .map(login_time)
            .collect::<Vec<_>>(),
        [last, first],
    );

    let window = ExportCursorWindow::new(None, 43, 10);
    let operation_batch = operations
        .find_export_batch("system", operation_filter, &scope, window)
        .await
        .map_err(|error| error.to_string())?;
    let login_batch = logins
        .find_export_batch("system", login_filter, &scope, window)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(
        operation_batch
            .iter()
            .map(operation_time)
            .collect::<Vec<_>>(),
        [first, last]
    );
    assert_eq!(
        login_batch.iter().map(login_time).collect::<Vec<_>>(),
        [first, last]
    );
    verify_log_export_summaries(cluster, &actor, BEGIN_TIME, END_TIME).await
}

async fn verify_log_export_summaries(
    cluster: &ControlDatabaseCluster,
    actor: &ActorContext,
    begin_time: &str,
    end_time: &str,
) -> Result<(), String> {
    let operation =
        OperLogExportFilter::new(None, None, Some(begin_time.into()), Some(end_time.into()))
            .map_err(|error| error.to_string())?;
    let login =
        LoginLogExportFilter::new(None, None, Some(begin_time.into()), Some(end_time.into()))
            .map_err(|error| error.to_string())?;
    let request = export::request(cluster.clone());
    let transaction = request.begin().await.map_err(|error| error.to_string())?;
    for selection in [
        ExportSelection::OperLogs(operation),
        ExportSelection::LoginLogs(login),
    ] {
        let snapshot = transaction
            .summarize_selection(&actor.tenant_id, actor, &selection)
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!((snapshot.matched_rows, snapshot.upper_id), (2, Some(42)));
    }
    transaction
        .rollback()
        .await
        .map_err(|error| error.to_string())
}

async fn assert_lossless(database: &DatabaseConnection) -> Result<(), String> {
    let row = database.query_one_raw(Statement::from_string(DbBackend::MySql,
        "SELECT m.name,p.name AS permission_name,parent.route_key AS parent_key FROM sys_menu m JOIN sys_permission p ON p.id=m.perm_id JOIN sys_menu parent ON parent.id=m.parent_id WHERE m.tenant_id='system' AND m.route_key='order.order'"))
        .await.map_err(db_error)?.ok_or("业务页面未初始化")?;
    assert_eq!(
        row.try_get::<String>("", "name")
            .map_err(|error| error.to_string())?,
        LABEL
    );
    assert_eq!(
        row.try_get::<String>("", "permission_name")
            .map_err(|error| error.to_string())?,
        format!("{LABEL}：查询")
    );
    assert_eq!(
        row.try_get::<String>("", "parent_key")
            .map_err(|error| error.to_string())?,
        "order"
    );
    require_count(database, "SELECT COUNT(*) AS value FROM sys_menu WHERE tenant_id='system' AND route_key='order' AND menu_type='M' AND perm_id IS NULL", 1).await
}

async fn verify_drift_is_read_only(database: &DatabaseConnection) -> Result<(), String> {
    for mutation in [
        "UPDATE sys_menu SET parent_id=NULL WHERE route_key='order.order'",
        "UPDATE sys_menu SET perm_id=NULL WHERE route_key='order.order'",
        "UPDATE sys_menu SET del_flag='1' WHERE route_key='order.order'",
    ] {
        execute(database, mutation).await?;
        let before = exact_inventory(database).await?;
        let error = migration::verify(database)
            .await
            .expect_err("业务目录漂移必须阻止启动");
        assert!(error.to_string().contains("业务菜单"));
        assert!(error.to_string().contains("control status"));
        assert_eq!(
            exact_inventory(database).await?,
            before,
            "verify 不得静默修复菜单"
        );
        // 测试已核对完整目录前后像后，显式模拟开发者审阅并运行 up。
        migration::up(database, None).await.map_err(db_error)?;
        migration::verify(database).await.map_err(db_error)?;
    }
    Ok(())
}

async fn verify_missing_is_read_only(database: &DatabaseConnection) -> Result<(), String> {
    execute(
        database,
        "DELETE FROM sys_menu WHERE route_key='order.order'",
    )
    .await?;
    execute(
        database,
        "DELETE FROM sys_permission WHERE code='order:order:list' AND tenant_id='system'",
    )
    .await?;
    let before = exact_inventory(database).await?;
    let error = migration::verify(database)
        .await
        .expect_err("缺少业务权限必须阻止启动");
    assert!(error.to_string().contains("order:order:list"));
    assert_eq!(exact_inventory(database).await?, before);
    migration::up(database, None).await.map_err(db_error)?;
    migration::verify(database).await.map_err(db_error)
}

async fn inventory(database: &DatabaseConnection) -> Result<Vec<Vec<String>>, String> {
    read_inventory(database, &[
        "SELECT CAST(JSON_OBJECT('tenant',tenant_id,'code',code,'name',name,'status',status) AS CHAR) AS item FROM sys_permission ORDER BY tenant_id,code",
        "SELECT CAST(JSON_OBJECT('tenant',m.tenant_id,'route',m.route_key,'name',m.name,'type',m.menu_type,'parent',parent.route_key,'permission',p.code,'visible',m.visible,'status',m.status,'deleted',m.del_flag) AS CHAR) AS item FROM sys_menu m LEFT JOIN sys_permission p ON p.id=m.perm_id LEFT JOIN sys_menu parent ON parent.id=m.parent_id ORDER BY m.tenant_id,m.route_key",
        "SELECT version AS item FROM seaql_migrations ORDER BY version",
    ]).await
}

async fn exact_inventory(database: &DatabaseConnection) -> Result<Vec<Vec<String>>, String> {
    read_inventory(database, &[
        "SELECT CAST(JSON_OBJECT('id',id,'tenant_id',tenant_id,'name',name,'code',code,'parent_id',parent_id,'perm_type',perm_type,'icon',icon,'sort',sort,'status',status,'created_at',created_at,'updated_at',updated_at) AS CHAR) AS item FROM sys_permission ORDER BY id",
        "SELECT CAST(JSON_OBJECT('id',id,'tenant_id',tenant_id,'name',name,'parent_id',parent_id,'menu_type',menu_type,'perm_id',perm_id,'route_key',route_key,'icon',icon,'sort',sort,'visible',visible,'status',status,'remark',remark,'del_flag',del_flag,'created_at',created_at,'updated_at',updated_at) AS CHAR) AS item FROM sys_menu ORDER BY id",
        "SELECT CAST(JSON_OBJECT('version',version,'applied_at',applied_at) AS CHAR) AS item FROM seaql_migrations ORDER BY version",
    ]).await
}

async fn read_inventory(
    database: &DatabaseConnection,
    queries: &[&str],
) -> Result<Vec<Vec<String>>, String> {
    let mut result = Vec::new();
    for sql in queries {
        let rows = database
            .query_all_raw(Statement::from_string(DbBackend::MySql, *sql))
            .await
            .map_err(db_error)?;
        result.push(
            rows.into_iter()
                .map(|row| {
                    row.try_get::<String>("", "item")
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    Ok(result)
}
