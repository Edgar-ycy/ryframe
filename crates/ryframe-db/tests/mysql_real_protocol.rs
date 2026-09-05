use std::{sync::Arc, time::Duration};

use ryframe_application::ports::users::UserQueryReadPort;
use ryframe_config::DbTlsMode;
use ryframe_db::{ControlDatabaseCluster, application_ports, connection, generated};
use ryframe_kernel::{AppError, DataScope, DataScopeContext, PaginationPolicy, ValidatedPageQuery};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, TransactionTrait};

#[path = "support/mysql.rs"]
mod mysql;
use mysql::{
    database_config_with_tls, db_error, execute, integration_enabled, require_count, run_mysql_test,
};

#[cfg(feature = "migration")]
#[path = "mysql_real_protocol/job_attempts.rs"]
mod job_attempts;
#[path = "mysql_real_protocol/linked_job_states.rs"]
mod linked_job_states;

const TLS_ENABLE_ENV: &str = "RYFRAME_MYSQL_TLS_INTEGRATION";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn required_tls_negotiates_a_real_mysql_cipher() {
    if !integration_enabled(TLS_ENABLE_ENV) {
        eprintln!("跳过 MySQL TLS 真实协议测试：设置 {TLS_ENABLE_ENV}=1 后才会连接外部服务");
        return;
    }

    let database = connection::connect(&database_config_with_tls("mysql", DbTlsMode::Required))
        .await
        .unwrap_or_else(|error| panic!("使用 required TLS 连接 MySQL 失败: {error}"));
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SHOW SESSION STATUS LIKE 'Ssl_cipher'",
        ))
        .await
        .unwrap_or_else(|error| panic!("读取 MySQL TLS 会话状态失败: {error}"))
        .unwrap_or_else(|| panic!("MySQL 未返回 Ssl_cipher 会话状态"));
    let cipher = row
        .try_get::<String>("", "Value")
        .unwrap_or_else(|error| panic!("读取 MySQL Ssl_cipher 失败: {error}"));
    database
        .close()
        .await
        .unwrap_or_else(|error| panic!("关闭 MySQL TLS 测试连接失败: {error}"));
    assert!(
        !cipher.trim().is_empty(),
        "MySQL required TLS 未协商出密码套件"
    );
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
async fn generated_post_port_enforces_tenant_isolation() {
    run_mysql_test("tenant", |database| async move {
        create_generated_post_fixture(&database).await?;
        verify_generated_post_reads(&database).await?;
        verify_generated_post_filters(&database).await?;
        verify_generated_post_writes(&database).await?;
        Ok(())
    })
    .await;
}

async fn create_generated_post_fixture(database: &DatabaseConnection) -> Result<(), String> {
    execute(
        database,
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
        database,
        "INSERT INTO sys_post \
         (id, tenant_id, name, code, sort, status, remark, del_flag, created_at, updated_at) \
         VALUES \
         (101, 'tenant-a', '岗位 A', 'shared-code', 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (102, 'tenant-a', '岗位停用', 'disabled-code', 2, '0', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (202, 'tenant-b', '岗位 B', 'shared-code', 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
    )
    .await
}

async fn verify_generated_post_reads(database: &DatabaseConnection) -> Result<(), String> {
    let persistence = generated::post::port(ControlDatabaseCluster::single(database.clone()));
    let tenant_a = persistence
        .find_by_id("tenant-a", 101)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "tenant-a 未读取到自己的岗位".to_owned())?;
    let tenant_b = persistence
        .find_by_id("tenant-b", 202)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "tenant-b 未读取到自己的岗位".to_owned())?;
    if tenant_a.id != 101 || tenant_b.id != 202 {
        return Err(format!(
            "岗位仓储返回了错误租户数据: tenant-a={}, tenant-b={}",
            tenant_a.id, tenant_b.id
        ));
    }
    let cross_tenant = persistence
        .find_by_id("tenant-a", 202)
        .await
        .map_err(|error| error.to_string())?;
    if cross_tenant.is_some() {
        return Err("岗位仓储允许 tenant-a 按主键读取 tenant-b 数据".to_owned());
    }
    Ok(())
}

async fn verify_generated_post_filters(database: &DatabaseConnection) -> Result<(), String> {
    let persistence = generated::post::port(ControlDatabaseCluster::single(database.clone()));
    let page = ValidatedPageQuery::new(1, 10, PaginationPolicy::new(10, 100))
        .map_err(|error| error.to_string())?;
    let list = |status| ryframe_application::generated::post::PostFilter {
        name: None,
        code: None,
        status,
    };
    let without_filter = persistence
        .find_by_page("tenant-a", page, list(None))
        .await
        .map_err(|error| error.to_string())?;
    let empty_filter = persistence
        .find_by_page("tenant-a", page, list(Some("")))
        .await
        .map_err(|error| error.to_string())?;
    let active_only = persistence
        .find_by_page("tenant-a", page, list(Some("1")))
        .await
        .map_err(|error| error.to_string())?;
    if without_filter.total != 2 || empty_filter.total != 2 || active_only.total != 1 {
        return Err(format!(
            "岗位状态过滤语义不一致: none={}, empty={}, active={}",
            without_filter.total, empty_filter.total, active_only.total
        ));
    }
    Ok(())
}

async fn verify_generated_post_writes(database: &DatabaseConnection) -> Result<(), String> {
    let persistence = generated::post::port(ControlDatabaseCluster::single(database.clone()));
    let tenant_b = persistence
        .find_by_id("tenant-b", 202)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "tenant-b 未读取到自己的岗位".to_owned())?;
    let mut cross_tenant_insert = tenant_b.clone();
    cross_tenant_insert.id = 303;
    cross_tenant_insert.code = "cross-tenant-insert".into();
    let insert_transaction = persistence
        .begin("tenant-a")
        .await
        .map_err(|error| error.to_string())?;
    let insert_error = insert_transaction
        .insert(cross_tenant_insert)
        .await
        .expect_err("跨租户岗位不得写入");
    if !matches!(
        &insert_error,
        AppError::Authorization(message) if message == "岗位事务租户不匹配"
    ) {
        return Err(format!("跨租户岗位写入返回了错误类型: {insert_error}"));
    }
    insert_transaction
        .rollback()
        .await
        .map_err(|error| error.to_string())?;
    if persistence
        .find_by_id("tenant-b", 303)
        .await
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err("跨租户岗位写入拒绝后仍产生了数据".into());
    }

    let update_transaction = persistence
        .begin("tenant-a")
        .await
        .map_err(|error| error.to_string())?;
    let update_error = update_transaction
        .update(tenant_b.clone())
        .await
        .expect_err("跨租户岗位不得更新");
    if !matches!(
        &update_error,
        AppError::Authorization(message) if message == "岗位事务租户不匹配"
    ) {
        return Err(format!("跨租户岗位更新返回了错误类型: {update_error}"));
    }
    update_transaction
        .rollback()
        .await
        .map_err(|error| error.to_string())?;
    let unchanged = persistence
        .find_by_id("tenant-b", 202)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "跨租户更新拒绝后岗位丢失".to_owned())?;
    if unchanged.name != tenant_b.name || unchanged.code != tenant_b.code {
        return Err("跨租户更新拒绝后岗位内容发生变化".into());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_detail_loads_department_with_tenant_and_data_scope() {
    run_mysql_test("user-department", |database| async move {
        create_user_detail_tables(&database).await?;
        seed_user_detail_rows(&database).await?;

        let query: Arc<dyn UserQueryReadPort> =
            application_ports::users::query(ControlDatabaseCluster::single(database.clone()));
        let all = DataScopeContext::super_admin(900);
        let detail = query
            .detail("tenant-a", 101, &all)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "未读取到 tenant-a 用户详情".to_owned())?;
        let department = detail
            .department
            .ok_or_else(|| "用户详情缺少同租户部门".to_owned())?;
        if department.id != 10
            || department.name != "研发部"
            || department.parent_id != Some(1)
            || department.ancestors != "0,1"
            || department.sort != 7
            || department.remark.as_deref() != Some("核心研发")
            || detail.user.dept_name.as_deref() != Some("研发部")
        {
            return Err(format!("用户详情部门投影不完整：{department:?}"));
        }
        if detail.roles.len() != 1
            || detail.roles[0].id != 301
            || detail.roles[0].code != "developer"
        {
            return Err(format!("用户详情角色不完整：{:?}", detail.roles));
        }

        for (user_id, label) in [(102, "无部门"), (103, "跨租户部门"), (104, "已删除部门")]
        {
            let detail = query
                .detail("tenant-a", user_id, &all)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{label}用户详情不存在"))?;
            if detail.department.is_some() || detail.user.dept_name.is_some() {
                return Err(format!("{label}用户泄漏了部门信息"));
            }
        }
        if query
            .detail("tenant-a", 201, &all)
            .await
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Err("tenant-a 可以读取 tenant-b 用户".into());
        }

        let self_scope = data_scope(DataScope::SelfOnly, 101, Some(10));
        if query
            .detail("tenant-a", 101, &self_scope)
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("SelfOnly 无法读取本人".into());
        }
        let other_self = data_scope(DataScope::SelfOnly, 102, None);
        if query
            .detail("tenant-a", 101, &other_self)
            .await
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Err("SelfOnly 可以读取其他用户".into());
        }
        let department_scope = data_scope(DataScope::Dept, 900, Some(10));
        if query
            .detail("tenant-a", 101, &department_scope)
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("部门范围无法读取本部门用户".into());
        }
        let denied_scope = data_scope(DataScope::Custom, 900, None);
        if query
            .detail("tenant-a", 101, &denied_scope)
            .await
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Err("空自定义范围可以读取用户".into());
        }
        Ok(())
    })
    .await;
}

async fn create_user_detail_tables(database: &DatabaseConnection) -> Result<(), String> {
    for sql in [
        "CREATE TABLE sys_dept (\
            id BIGINT NOT NULL PRIMARY KEY, tenant_id VARCHAR(64) NOT NULL,\
            name VARCHAR(64) NOT NULL, parent_id BIGINT NULL, ancestors VARCHAR(512) NOT NULL,\
            sort INT NOT NULL, status VARCHAR(8) NOT NULL, remark VARCHAR(255) NULL,\
            del_flag VARCHAR(8) NOT NULL, created_at DATETIME(6) NOT NULL,\
            updated_at DATETIME(6) NOT NULL\
        ) ENGINE=InnoDB",
        "CREATE TABLE sys_user (\
            id BIGINT NOT NULL PRIMARY KEY, tenant_id VARCHAR(64) NOT NULL,\
            username VARCHAR(64) NOT NULL, password_hash VARCHAR(255) NOT NULL,\
            nickname VARCHAR(64) NOT NULL, email VARCHAR(128) NOT NULL, phone VARCHAR(32) NOT NULL,\
            avatar VARCHAR(512) NULL, avatar_file_id BIGINT NULL, preferred_locale VARCHAR(32) NULL,\
            status VARCHAR(32) NOT NULL, authorization_version INT NOT NULL, dept_id BIGINT NULL,\
            remark VARCHAR(255) NULL, login_ip VARCHAR(64) NULL, login_date DATETIME(6) NULL,\
            del_flag VARCHAR(8) NOT NULL, created_at DATETIME(6) NOT NULL,\
            updated_at DATETIME(6) NOT NULL\
        ) ENGINE=InnoDB",
        "CREATE TABLE sys_role (\
            id BIGINT NOT NULL PRIMARY KEY, tenant_id VARCHAR(64) NOT NULL, name VARCHAR(64) NOT NULL,\
            code VARCHAR(64) NOT NULL, is_super TINYINT NOT NULL, data_scope VARCHAR(8) NOT NULL,\
            status VARCHAR(8) NOT NULL, sort INT NOT NULL, remark VARCHAR(255) NULL,\
            del_flag VARCHAR(8) NOT NULL, created_at DATETIME(6) NOT NULL,\
            updated_at DATETIME(6) NOT NULL\
        ) ENGINE=InnoDB",
        "CREATE TABLE sys_user_role (\
            tenant_id VARCHAR(64) NOT NULL, user_id BIGINT NOT NULL, role_id BIGINT NOT NULL,\
            PRIMARY KEY (tenant_id, user_id, role_id)\
        ) ENGINE=InnoDB",
    ] {
        execute(database, sql).await?;
    }
    Ok(())
}

async fn seed_user_detail_rows(database: &DatabaseConnection) -> Result<(), String> {
    execute(
        database,
        "INSERT INTO sys_dept \
         (id, tenant_id, name, parent_id, ancestors, sort, status, remark, del_flag, created_at, updated_at) VALUES \
         (10, 'tenant-a', '研发部', 1, '0,1', 7, '1', '核心研发', '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (11, 'tenant-a', '已删除部门', NULL, '0', 8, '1', NULL, '2', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (20, 'tenant-b', '其他租户部门', NULL, '0', 9, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
    )
    .await?;
    execute(
        database,
        "INSERT INTO sys_user \
         (id, tenant_id, username, password_hash, nickname, email, phone, avatar, avatar_file_id, \
          preferred_locale, status, authorization_version, dept_id, remark, login_ip, login_date, \
          del_flag, created_at, updated_at) VALUES \
         (101, 'tenant-a', 'alice', 'hash', 'Alice', '', '', NULL, NULL, NULL, '1', 0, 10, NULL, NULL, NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (102, 'tenant-a', 'bob', 'hash', 'Bob', '', '', NULL, NULL, NULL, '1', 0, NULL, NULL, NULL, NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (103, 'tenant-a', 'carol', 'hash', 'Carol', '', '', NULL, NULL, NULL, '1', 0, 20, NULL, NULL, NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (104, 'tenant-a', 'dave', 'hash', 'Dave', '', '', NULL, NULL, NULL, '1', 0, 11, NULL, NULL, NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (201, 'tenant-b', 'eve', 'hash', 'Eve', '', '', NULL, NULL, NULL, '1', 0, 20, NULL, NULL, NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
    )
    .await?;
    execute(
        database,
        "INSERT INTO sys_role \
         (id, tenant_id, name, code, is_super, data_scope, status, sort, remark, del_flag, created_at, updated_at) \
         VALUES (301, 'tenant-a', '开发者', 'developer', 0, '3', '1', 1, NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
    )
    .await?;
    execute(
        database,
        "INSERT INTO sys_user_role (tenant_id, user_id, role_id) VALUES ('tenant-a', 101, 301)",
    )
    .await
}

fn data_scope(scope: DataScope, user_id: i64, dept_id: Option<i64>) -> DataScopeContext {
    DataScopeContext {
        scope,
        user_id,
        dept_id,
        ancestors: None,
        custom_dept_ids: Vec::new(),
        include_self: false,
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
