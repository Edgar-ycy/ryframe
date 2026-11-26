use axum::{Router, middleware::from_fn_with_state};
use ryframe_api::{
    AppState,
    auth_middleware::auth_middleware,
    handlers::{profile_handler::profile_router, tenant_handler::tenant_router},
};
use ryframe_application::{AuthService, PrincipalResolver};
use ryframe_auth::jwt::{Claims, decode_token};
use ryframe_kernel::AppError;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use super::{
    NEW_PASSWORD,
    account_http::{Response, Server},
    initialize, message, mysql, role_assignment, sql_initialization,
};

const TENANT_ID: &str = "tenant-expiry-test";
const TENANTS: &str = "/api/v1/platform/tenants";
const TENANT: &str = "/api/v1/platform/tenants/tenant-expiry-test";
const TENANT_ADMIN: &str = "tenant_expiry_admin";
const TENANT_PASSWORD: &str = "ExpiryFixture123!";
const JSON_HEADERS: &str = "Content-Type: application/json\r\n";
const CREATE_HEADERS: &str =
    "Content-Type: application/json\r\nIdempotency-Key: tenant-expiry-rfc3339-replay\r\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tenant_expiry_round_trips_and_preserves_session_version_through_http() {
    initialize();
    mysql::run_mysql_test("tenant-expiry-http", |database| async move {
        let exported = sql_initialization::import(&database).await?;
        let (state, _lifetime) = role_assignment::state(&database).await?;
        let router = Router::new()
            .nest(TENANTS, tenant_router(state.clone()))
            .nest(
                "/api/v1/auth/profile",
                profile_router().with_state(state.clone()),
            )
            .layer(from_fn_with_state(state.auth.clone(), auth_middleware));
        let server = Server::start_router(router).await?;
        verify_expiry(&database, &state, &server).await?;
        drop(server);
        drop(state);
        exported.finish()
    })
    .await;
}

async fn verify_expiry(
    database: &DatabaseConnection,
    state: &AppState,
    server: &Server,
) -> Result<(), String> {
    let auth = &state.services.identity.auth;
    let token = platform_token(auth, server).await?;
    let now = database_utc_second(database).await?;
    let future = now + Duration::days(30);
    let past = now - Duration::days(1);
    let clock_offset = UtcOffset::from_hms(5, 30, 0).map_err(message)?;
    let non_utc = future
        .to_offset(clock_offset)
        .format(&Rfc3339)
        .map_err(message)?;
    let utc = future.format(&Rfc3339).map_err(message)?;
    let first = create(server, &token, &non_utc).await?;
    let (id, initial_version) = require_image(database, &first, Some(future)).await?;
    let repeated = create(server, &token, &utc).await?;
    assert_eq!(
        require_image(database, &repeated, Some(future)).await?,
        (id, initial_version)
    );
    assert_eq!(repeated.body["data"], first.body["data"]);

    let service = &state.services.platform.tenant;
    let choices = service
        .login_choices("", 1, 1, Some(TENANT_ID))
        .await
        .map_err(message)?;
    assert_eq!(choices.items.len(), 1);
    assert_eq!(choices.items[0].tenant_id, TENANT_ID);

    let original = tenant_claims(auth, initial_version).await?;
    let equal = update(server, &token, Some(json!(non_utc))).await?;
    let saved = require_image(database, &equal, Some(future)).await?;
    assert_eq!(saved, (id, initial_version));
    auth.resolve_principal(&original).await.map_err(message)?;

    let omitted = update(server, &token, None).await?;
    let saved = require_image(database, &omitted, None).await?;
    assert_eq!(saved, (id, initial_version + 1));
    require_stale_session(auth, &original).await;

    let restored = update(server, &token, Some(json!(utc))).await?;
    let saved = require_image(database, &restored, Some(future)).await?;
    assert_eq!(saved, (id, initial_version + 2));
    let refreshed = tenant_claims(auth, initial_version + 2).await?;
    let explicit_null = update(server, &token, Some(Value::Null)).await?;
    let saved = require_image(database, &explicit_null, None).await?;
    assert_eq!(saved, (id, initial_version + 3));
    require_stale_session(auth, &refreshed).await;

    let expired = update(
        server,
        &token,
        Some(json!(past.format(&Rfc3339).map_err(message)?)),
    )
    .await?;
    let saved = require_image(database, &expired, Some(past)).await?;
    assert_eq!(saved, (id, initial_version + 4));
    let choices = service
        .login_choices("", 1, 1, Some(TENANT_ID))
        .await
        .map_err(message)?;
    assert!(choices.items.is_empty());
    assert!(matches!(
        auth.login(TENANT_ID, TENANT_ADMIN, TENANT_PASSWORD).await,
        Err(AppError::Authentication(reason)) if reason == "租户已到期"
    ));
    Ok(())
}

async fn platform_token(auth: &AuthService, server: &Server) -> Result<String, String> {
    let initial = auth
        .login("system", "admin", sql_initialization::ADMIN_PASSWORD)
        .await
        .map_err(message)?;
    assert!(initial.user_info.is_super_admin);
    if !initial.user_info.password_change_required {
        return Ok(initial.access_token);
    }
    server
        .change_password(
            &initial.access_token,
            sql_initialization::ADMIN_PASSWORD,
            NEW_PASSWORD,
        )
        .await?;
    auth.login("system", "admin", NEW_PASSWORD)
        .await
        .map(|session| session.access_token)
        .map_err(message)
}

async fn create(server: &Server, token: &str, expiry: &str) -> Result<Response, String> {
    let mut body = tenant_details();
    body["tenant_id"] = json!(TENANT_ID);
    body["expire_at"] = json!(expiry);
    body["admin_username"] = json!(TENANT_ADMIN);
    body["admin_password"] = json!(TENANT_PASSWORD);
    body["plan_version_id"] = json!("1");
    body["data_target_key"] = json!("shared-control");
    server
        .request(
            "POST",
            TENANTS,
            token,
            CREATE_HEADERS,
            serde_json::to_vec(&body).map_err(message)?,
        )
        .await
}

async fn update(server: &Server, token: &str, expiry: Option<Value>) -> Result<Response, String> {
    let mut body = tenant_details();
    if let Some(expiry) = expiry {
        body["expire_at"] = expiry;
    }
    server
        .request(
            "PUT",
            TENANT,
            token,
            JSON_HEADERS,
            serde_json::to_vec(&body).map_err(message)?,
        )
        .await
}

fn tenant_details() -> Value {
    json!({
        "name": "租户到期时间验收",
        "max_users": 100,
        "max_roles": 20,
        "max_storage_mb": 1024,
        "max_requests_per_min": 1000,
    })
}

async fn database_utc_second(database: &DatabaseConnection) -> Result<OffsetDateTime, String> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', UTC_TIMESTAMP()) AS utc_second",
        ))
        .await
        .map_err(message)?
        .ok_or("MySQL 未返回 UTC 时间")?;
    OffsetDateTime::from_unix_timestamp(row.try_get("", "utc_second").map_err(message)?)
        .map_err(message)
}

async fn require_image(
    database: &DatabaseConnection,
    response: &Response,
    expiry: Option<OffsetDateTime>,
) -> Result<(i64, i32), String> {
    assert_eq!(
        response.status, 200,
        "租户响应 error_key={}",
        response.body["error_key"]
    );
    let data = &response.body["data"];
    assert_eq!(data["tenant_id"], TENANT_ID);
    assert_eq!(data["status"], "enabled");
    let expected = expiry
        .map(|at| at.format(&Rfc3339))
        .transpose()
        .map_err(message)?;
    assert_eq!(data.get("expire_at"), Some(&json!(expected)));
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id, status, DATE_FORMAT(expire_at, '%Y-%m-%dT%H:%i:%sZ') AS expires, session_version FROM sys_tenant WHERE tenant_id=?",
            [TENANT_ID.into()],
        ))
        .await
        .map_err(message)?
        .ok_or("未找到当前验收租户")?;
    assert_eq!(
        row.try_get::<String>("", "status").map_err(message)?,
        "enabled"
    );
    assert_eq!(
        row.try_get::<Option<String>>("", "expires")
            .map_err(message)?,
        expected
    );
    Ok((
        row.try_get("", "id").map_err(message)?,
        row.try_get("", "session_version").map_err(message)?,
    ))
}

async fn tenant_claims(auth: &AuthService, session_version: i32) -> Result<Claims, String> {
    let login = auth
        .login(TENANT_ID, TENANT_ADMIN, TENANT_PASSWORD)
        .await
        .map_err(message)?;
    let claims = decode_token(&login.access_token, &auth.token_settings()).map_err(message)?;
    assert_eq!(claims.tenant_session_version, session_version);
    auth.resolve_principal(&claims).await.map_err(message)?;
    Ok(claims)
}

async fn require_stale_session(auth: &AuthService, claims: &Claims) {
    assert!(matches!(
        auth.resolve_principal(claims).await,
        Err(AppError::Authentication(reason)) if reason == "租户会话已失效，请重新登录"
    ));
}
