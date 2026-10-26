use futures_util::FutureExt;
use ryframe_api::AppState;
use ryframe_application::{
    JobRunResult, JobWorkerPolicy, ports::jobs::ExecutionTenantScope,
    system::identity::UserImportData,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

use super::{initialize, message, mysql, role_assignment, sql_initialization};

use super::account_http::Server;

const USERS: [&str; 2] = ["import_account_a", "import_account_b"];
const CHOSEN_PASSWORDS: [&str; 2] = ["ImportedAccountA123!", "ImportedAccountB123!"];
const ADMIN_PASSWORD: &str = "ImportAdministrator123!";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn imported_accounts_login_with_initial_password_and_complete_required_change() {
    if !mysql::integration_enabled() {
        eprintln!(
            "跳过批量导入账号 HTTP/MySQL 闭环（未验收）：必须存在显式 .local-tests/mysql/config 配置目录"
        );
        return;
    }
    initialize();
    mysql::run_mysql_test("import-accounts", |database| async move {
        let exported = sql_initialization::import(&database).await?;
        let (state, lifetime) = role_assignment::state(&database).await?;
        let result = std::panic::AssertUnwindSafe(exercise(&database, &state))
            .catch_unwind()
            .await;
        drop(state);
        match result {
            Ok(Ok(())) => {
                drop(lifetime);
                exported.finish()
            }
            Ok(Err(error)) => {
                lifetime.preserve();
                Err(error)
            }
            Err(payload) => {
                lifetime.preserve();
                std::panic::resume_unwind(payload)
            }
        }
    })
    .await;
}

async fn exercise(database: &DatabaseConnection, state: &AppState) -> Result<(), String> {
    let server = Server::start(state.clone()).await?;
    let initial = state
        .services
        .identity
        .auth
        .login("system", "admin", sql_initialization::ADMIN_PASSWORD)
        .await
        .map_err(message)?;
    assert!(initial.user_info.password_change_required);
    server
        .change_password(
            &initial.access_token,
            sql_initialization::ADMIN_PASSWORD,
            ADMIN_PASSWORD,
        )
        .await?;
    let admin = state
        .services
        .identity
        .auth
        .login("system", "admin", ADMIN_PASSWORD)
        .await
        .map_err(message)?;
    let bytes = workbook(database).await?;
    let submitted = server.upload(&admin.access_token, bytes).await?;
    assert_eq!(submitted.status, 202);
    let import_id = submitted.body["data"]["id"]
        .as_str()
        .ok_or("导入响应缺少字符串任务 ID")?
        .parse::<i64>()
        .map_err(message)?;
    execute_claimed_import(database, state, import_id).await?;
    let result = server
        .get(
            &format!("/api/v1/system/user-imports/{import_id}"),
            &admin.access_token,
        )
        .await?;
    assert_eq!(result.status, 200);
    let job = &result.body["data"];
    assert_eq!(job["status"], "succeeded");
    for field in ["total_rows", "processed_rows", "success_count"] {
        assert_eq!(job[field], 2);
    }
    for field in ["skipped_count", "failure_count"] {
        assert_eq!(job[field], 0);
    }
    assert!(!job["completed_at"].is_null());
    for (username, chosen) in USERS.into_iter().zip(CHOSEN_PASSWORDS) {
        verify_account(database, state, &server, username, chosen).await?;
    }
    Ok(())
}

pub(super) async fn workbook(database: &DatabaseConnection) -> Result<Vec<u8>, String> {
    let department = database.query_one_raw(Statement::from_string(DbBackend::MySql,
        "SELECT name FROM sys_dept WHERE tenant_id='system' AND parent_id IS NULL AND status='1' AND del_flag='0' ORDER BY id LIMIT 1"))
        .await.map_err(message)?.ok_or("正式 SQL 未初始化可用根部门")?;
    let path: String = department.try_get("", "name").map_err(message)?;
    let rows = USERS
        .into_iter()
        .enumerate()
        .map(|(index, username)| {
            serde_json::json!({
                "username": username, "nickname": format!("导入普通用户{}", index + 1),
                "email": format!("{username}@example.com"), "phone": "", "department_path": path,
            })
        })
        .collect();
    ryframe::boot::spreadsheet::document_processor()
        .export_rows(rows, "用户数据", UserImportData::excel_headers())
        .await
        .map_err(message)
}

pub(super) async fn execute_claimed_import(
    database: &DatabaseConnection,
    state: &AppState,
    import_id: i64,
) -> Result<(), String> {
    let policy =
        JobWorkerPolicy::new(Some("account-import"), 600, 5, 10, 100, 60, 1).map_err(message)?;
    let worker = ryframe::boot::jobs::build_job_worker(
        state.services.operations.job_queue.clone(),
        &policy,
        ExecutionTenantScope::tenant_and_platform("system"),
        ryframe::boot::jobs::JobWorkerDependencies::from_api_services(
            &state.services,
            None,
            state.settings.messaging.enabled,
        ),
    )
    .map_err(message)?;
    assert_eq!(
        worker
            .run_once("account-import-worker")
            .await
            .map_err(message)?,
        JobRunResult::Succeeded
    );
    let row = database.query_one_raw(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT j.status,j.job_type,CAST(j.payload AS CHAR) AS payload FROM sys_background_job j JOIN sys_user_import_job i ON i.background_job_id=j.id AND i.tenant_id=j.tenant_id WHERE i.tenant_id='system' AND i.id=?",
        [import_id.into()])).await.map_err(message)?.ok_or("导入缺少关联后台任务")?;
    assert_eq!(
        row.try_get::<String>("", "status").map_err(message)?,
        "succeeded"
    );
    assert_eq!(
        row.try_get::<String>("", "job_type").map_err(message)?,
        "system.user.import"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&row.try_get::<String>("", "payload").map_err(message)?)
            .map_err(message)?;
    assert_eq!(payload["import_job_id"], import_id.to_string());
    Ok(())
}

async fn verify_account(
    database: &DatabaseConnection,
    state: &AppState,
    server: &Server,
    username: &str,
    chosen: &str,
) -> Result<(), String> {
    let id = require_user(database, username, true).await?;
    let auth = &state.services.identity.auth;
    let initial = auth
        .login("system", username, "123456")
        .await
        .map_err(message)?;
    assert_eq!(initial.user_id, id);
    assert!(initial.user_info.password_change_required);
    assert!(!initial.user_info.is_super_admin);
    assert!(initial.user_info.roles.is_empty());
    assert!(initial.user_info.perms.is_empty());
    let profile = server
        .get("/api/v1/auth/profile", &initial.access_token)
        .await?;
    assert_eq!(profile.status, 403);
    let business = server
        .get("/api/v1/system/users", &initial.access_token)
        .await?;
    assert_eq!(business.status, 403);
    assert_eq!(business.body["error_key"], "permission_denied");
    server
        .change_password(&initial.access_token, "123456", chosen)
        .await?;
    require_user(database, username, false).await?;
    assert!(matches!(
        auth.login("system", username, "123456").await,
        Err(ryframe_kernel::AppError::Authentication(_))
    ));
    assert_eq!(
        server
            .get("/api/v1/auth/profile", &initial.access_token)
            .await?
            .status,
        401
    );
    assert!(matches!(
        auth.refresh_token(
            &initial.refresh_token,
            "6d934bca-b4cd-4dbf-b058-21c5683b727e"
        )
        .await,
        Err(ryframe_kernel::AppError::Authentication(_))
    ));
    let changed = auth
        .login("system", username, chosen)
        .await
        .map_err(message)?;
    assert!(!changed.user_info.password_change_required);
    assert!(!changed.user_info.is_super_admin);
    assert!(changed.user_info.roles.is_empty());
    assert!(changed.user_info.perms.is_empty());
    let profile = server
        .get("/api/v1/auth/profile", &changed.access_token)
        .await?;
    assert_eq!(profile.status, 200);
    assert_eq!(profile.body["data"]["username"], username);
    assert_eq!(
        server
            .get("/api/v1/system/users", &changed.access_token)
            .await?
            .status,
        403
    );
    Ok(())
}

async fn require_user(
    database: &DatabaseConnection,
    username: &str,
    required: bool,
) -> Result<i64, String> {
    let row = database.query_one_raw(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT id,status,CAST(password_change_required AS SIGNED) AS required_change,CAST((SELECT COUNT(*) FROM sys_user_role r WHERE r.tenant_id=u.tenant_id AND r.user_id=u.id) AS SIGNED) AS role_count FROM sys_user u WHERE tenant_id='system' AND username=? AND del_flag='0'",
        [username.into()])).await.map_err(message)?.ok_or("导入未创建预期普通账号")?;
    assert_eq!(row.try_get::<String>("", "status").map_err(message)?, "1");
    assert_eq!(
        row.try_get::<i64>("", "required_change").map_err(message)?,
        i64::from(required)
    );
    assert_eq!(row.try_get::<i64>("", "role_count").map_err(message)?, 0);
    row.try_get("", "id").map_err(message)
}
