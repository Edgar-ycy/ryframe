use super::{
    mysql,
    ownership::{self, ManagedChild},
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use std::time::Duration;

const LOCKS: &str = "SELECT CAST(IS_USED_LOCK(SHA2(CONCAT('ryframe:migration:',DATABASE()),256)) AS SIGNED), \
    CAST(IS_USED_LOCK(SHA2(CONCAT('ryframe:tenant-data-migration:',DATABASE()),256)) AS SIGNED)";
const HOLD_TENANT_LOCK: &str = "SELECT CAST(CONNECTION_ID() AS SIGNED), \
    GET_LOCK(SHA2(CONCAT('ryframe:tenant-data-migration:',DATABASE()),256),0)";
const RELEASE_TENANT_LOCK: &str =
    "SELECT RELEASE_LOCK(SHA2(CONCAT('ryframe:tenant-data-migration:',DATABASE()),256))";
const TABLES: &str =
    "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE()";

async fn locks(database: &DatabaseConnection) -> Result<(Option<i64>, Option<i64>), String> {
    let row = database
        .query_one_raw(Statement::from_string(DbBackend::MySql, LOCKS))
        .await
        .map_err(mysql::db_error)?
        .ok_or("迁移锁观察无结果")?;
    Ok((
        row.try_get_by_index(0).map_err(mysql::db_error)?,
        row.try_get_by_index(1).map_err(mysql::db_error)?,
    ))
}

async fn wait_for_blocked_cli(
    database: &DatabaseConnection,
    cli: &mut ManagedChild,
    blocker: i64,
) -> Result<i64, String> {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            if !cli.is_running()? {
                return Err("正式迁移 CLI 在到达锁阻塞点前退出".into());
            }
            let (control, tenant) = locks(database).await?;
            if tenant != Some(blocker) {
                return Err("预先持有的租户迁移锁归属变化".into());
            }
            if let Some(owner) = control.filter(|owner| *owner != blocker) {
                let waiting = database
                    .query_one_raw(Statement::from_sql_and_values(
                        DbBackend::MySql,
                        "SELECT COUNT(*) FROM information_schema.processlist \
                         WHERE ID=? AND DB=DATABASE() AND STATE='User lock'",
                        [owner.into()],
                    ))
                    .await
                    .map_err(mysql::db_error)?
                    .ok_or("CLI 会话观察无结果")?;
                if waiting
                    .try_get_by_index::<i64>(0)
                    .map_err(mysql::db_error)?
                    == 1
                {
                    return Ok(owner);
                }
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .map_err(|_| "未在期限内观察到正式 CLI 已持有第一把锁并等待第二把锁".to_owned())?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_initialization_cli_releases_its_mysql_session_without_writes() {
    mysql::run_mysql_test("ownerclikill", |database| async move {
        let identity = ryframe_db::connection::physical_identity(&database)
            .await
            .map_err(mysql::db_error)?;
        let connection =
            mysql::MySqlTestConfiguration::load()?.connection(&identity.database, None)?;
        let config = ownership::config_for(connection)?;
        let blocker = database.begin().await.map_err(mysql::db_error)?;
        let row = blocker
            .query_one_raw(Statement::from_string(DbBackend::MySql, HOLD_TENANT_LOCK))
            .await
            .map_err(mysql::db_error)?
            .ok_or("租户迁移锁获取无结果")?;
        let blocker_id: i64 = row.try_get_by_index(0).map_err(mysql::db_error)?;
        if row.try_get_by_index::<i64>(1).map_err(mysql::db_error)? != 1 {
            return Err("无法固定正式 CLI 的第二把迁移锁".into());
        }
        let interrupted = async {
            let mut cli = ManagedChild::start(config.path(), &["control", "up"])?;
            let owner = wait_for_blocked_cli(&database, &mut cli, blocker_id).await?;
            mysql::require_count(&database, TABLES, 0).await?;
            cli.terminate()?;
            if cli.output().status.success() {
                return Err("被强杀的正式迁移 CLI 返回成功".into());
            }
            Ok::<_, String>(owner)
        }
        .await;
        let release = blocker
            .query_one_raw(Statement::from_string(
                DbBackend::MySql,
                RELEASE_TENANT_LOCK,
            ))
            .await
            .map_err(mysql::db_error)?
            .ok_or("原阻塞连接释放锁无结果")?;
        blocker.rollback().await.map_err(mysql::db_error)?;
        if release
            .try_get_by_index::<i64>(0)
            .map_err(mysql::db_error)?
            != 1
        {
            return Err("原阻塞连接未释放第二把迁移锁".into());
        }
        let owner = interrupted?;
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if locks(&database).await? == (None, None) {
                    let statement = Statement::from_sql_and_values(
                        DbBackend::MySql,
                        "SELECT COUNT(*) AS value FROM information_schema.processlist WHERE ID=?",
                        [owner.into()],
                    );
                    let row = database
                        .query_one_raw(statement)
                        .await
                        .map_err(mysql::db_error)?
                        .ok_or("原 CLI 会话后像无结果")?;
                    if row.try_get_by_index::<i64>(0).map_err(mysql::db_error)? == 0 {
                        return Ok::<_, String>(());
                    }
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .map_err(|_| "被强杀的 CLI 原会话或迁移锁仍未退役".to_owned())??;
        mysql::require_count(&database, TABLES, 0).await
    })
    .await;
}

async fn ownership_visible(database: &DatabaseConnection, scope: &str) -> Result<bool, String> {
    let count = database
        .query_one_raw(Statement::from_string(DbBackend::MySql,
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='ryframe_resource_ownership'"))
        .await.map_err(mysql::db_error)?.ok_or("归属表观察无结果")?;
    if count.try_get_by_index::<i64>(0).map_err(mysql::db_error)? == 0 {
        return Ok(false);
    }
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT scope_id,marker FROM ryframe_resource_ownership WHERE resource_kind='control'",
        ))
        .await
        .map_err(mysql::db_error)?;
    let Some(row) = row else {
        return Ok(false);
    };
    let actual_scope: String = row.try_get_by_index(0).map_err(mysql::db_error)?;
    let actual_marker: String = row.try_get_by_index(1).map_err(mysql::db_error)?;
    if actual_scope != scope
        || actual_marker != ryframe_db::resource_ownership::marker(scope, "control")
    {
        return Err("独立连接观察到外来归属".into());
    }
    Ok(true)
}

async fn lose_committed_response(
    gate: &mut super::response_gate::ResponseGate,
    database: &DatabaseConnection,
    cli: &mut ManagedChild,
    scope: &str,
) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(70), async {
        loop {
            if !cli.is_running()? {
                return Err("正式初始化 CLI 在归属提交前已退出".into());
            }
            let event =
                tokio::time::timeout(Duration::from_millis(150), gate.responses.recv()).await;
            if let Ok(permit) = event {
                let permit = permit.ok_or("初始化响应门控提前结束")?;
                let committed = ownership_visible(database, scope).await?;
                permit
                    .send(!committed)
                    .map_err(|_| "初始化响应无法交还门控")?;
                if committed {
                    return Ok(());
                }
            }
        }
    })
    .await
    .map_err(|_| "未观察到真实初始化归属 COMMIT 响应".to_owned())?
}

async fn completion_image(database: &DatabaseConnection) -> Result<Vec<(String, i64)>, String> {
    let rows = database
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT version,applied_at FROM seaql_migrations ORDER BY version",
        ))
        .await
        .map_err(mysql::db_error)?;
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get_by_index(0).map_err(mysql::db_error)?,
                row.try_get_by_index(1).map_err(mysql::db_error)?,
            ))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_initialization_commit_response_requires_readonly_reconciliation() {
    mysql::run_mysql_test("ownercliunknown", |database| async move {
        let identity = ryframe_db::connection::physical_identity(&database)
            .await
            .map_err(mysql::db_error)?;
        let mut connection =
            mysql::MySqlTestConfiguration::load()?.connection(&identity.database, None)?;
        let direct = ownership::config_for(connection.clone())?;
        let scope =
            ryframe_config::AppConfig::load(direct.path(), ryframe_config::Environment::Test)
                .map_err(|error| error.to_string())?
                .scope_id;
        mysql::require_count(&database, TABLES, 0).await?;
        let mut gate =
            super::response_gate::ResponseGate::start(connection.host.clone(), connection.port)
                .await
                .map_err(|error| error.to_string())?;
        connection.host = gate.address.ip().to_string();
        connection.port = gate.address.port();
        let outcome = async {
            let through_gate = ownership::config_for(connection)?;
            gate.arm();
            let mut cli = ManagedChild::start(through_gate.path(), &["control", "up"])?;
            lose_committed_response(&mut gate, &database, &mut cli, scope.as_str()).await?;
            let output = cli.output();
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            if output.status.success()
                || !diagnostic.contains("归属提交结果未知")
                || !diagnostic.contains("只读核对归属及初始化账本")
                || !diagnostic.contains("不自动重放")
            {
                return Err(format!("正式 CLI 未按未知归属提交关闭：{diagnostic}"));
            }
            Ok::<_, String>(())
        }
        .await;
        gate.stop().await?;
        outcome?;
        let before = completion_image(&database).await?;
        if before.is_empty() || !ownership_visible(&database, scope.as_str()).await? {
            return Err("原提交的权威归属或初始化账本不完整".into());
        }
        mysql::require_count(
            &database,
            "SELECT COUNT(*) AS value FROM ryframe_resource_ownership",
            1,
        )
        .await?;
        let verified = ownership::run(direct.path(), &["control", "verify"]);
        if !verified.status.success() {
            return Err("原提交的正式只读控制库校验失败".into());
        }
        if before != completion_image(&database).await?
            || !ownership_visible(&database, scope.as_str()).await?
        {
            return Err("只读核对改变了原提交的账本或归属".into());
        }
        Ok(())
    })
    .await;
}
