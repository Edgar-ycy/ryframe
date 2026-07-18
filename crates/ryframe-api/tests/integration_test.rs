//! API 集成测试
//!
//! 使用隔离 MySQL 8.4 数据库 + axum test client 测试端到端流程。

use std::sync::Arc;

#[path = "../../ryframe-db/tests/common/test_database.rs"]
mod test_database;

use test_database::TestDatabase;

use axum::{
    body::Body,
    extract::{ConnectInfo, Path, State},
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use ryframe_api::{
    AppServices, AppState, handlers::online_user_handler, router::api_router,
    runtime::RuntimeComponents,
};
use ryframe_config::{
    AppConfig, AppSettings, AuthConfig, DatabaseConfig, DbConnection, LoggerConfig, RateLimitConfig,
};
use ryframe_db::{
    DatabaseCluster,
    entities::{config, dept, permission, role, role_permission, tenant, user},
};
use ryframe_middleware::rate_limit::RateLimitState;
use ryframe_service::{
    AuthService,
    system::{
        CaptchaStore, ConfigService, DeptService, DictService, FileService, GeneratorService,
        LoginInfoService, MenuService, NoticeService, OnlineUserService, OperLogService,
        PermissionService, PostService, ProfileService, RoleService, TenantService, UserService,
    },
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Schema,
};
use std::net::SocketAddr;
use tower::ServiceExt;

/// 创建隔离的 MySQL 8.4 测试数据库。
async fn setup_test_db() -> TestDatabase {
    TestDatabase::create("api_full").await
}

/// 填充测试数据：管理员 + 部门
async fn create_all_tables(db: &DatabaseConnection) {
    let backend = DatabaseBackend::MySql;
    let schema = Schema::new(backend);

    macro_rules! create {
        ($entity:path) => {
            let stmt = schema.create_table_from_entity($entity);
            db.execute(&stmt).await.expect("create table failed");
        };
    }

    create!(ryframe_db::entities::tenant::Entity);
    create!(ryframe_db::entities::config::Entity);
    create!(ryframe_db::entities::dept::Entity);
    create!(ryframe_db::entities::dict_type::Entity);
    create!(ryframe_db::entities::dict_data::Entity);
    create!(ryframe_db::entities::login_info::Entity);
    create!(ryframe_db::entities::notice::Entity);
    create!(ryframe_db::entities::oper_log::Entity);
    create!(ryframe_db::entities::permission::Entity);
    create!(ryframe_db::entities::post::Entity);
    create!(ryframe_db::entities::role::Entity);
    create!(ryframe_db::entities::menu::Entity);
    create!(ryframe_db::entities::user::Entity);
    create!(ryframe_db::entities::password_reset_request::Entity);
    create!(ryframe_db::entities::user_role::Entity);
    create!(ryframe_db::entities::role_permission::Entity);
    create!(ryframe_db::entities::role_dept::Entity);
    create!(ryframe_db::entities::sys_file::Entity);
}

async fn seed_test_data(db: &DatabaseConnection) {
    create_all_tables(db).await;

    let system_tenant = tenant::Model {
        id: 1,
        tenant_id: "system".into(),
        name: "系统租户".into(),
        domain: None,
        status: tenant::Model::STATUS_NORMAL.into(),
        expire_at: None,
        max_users: 100,
        max_roles: 20,
        max_storage_mb: 1024,
        max_requests_per_min: 1000,
        session_version: 1,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    tenant::Entity::insert(tenant::ActiveModel::from(system_tenant))
        .exec(db)
        .await
        .unwrap();

    // 创建根部门
    let dept_model = dept::Model {
        id: 1,
        tenant_id: "system".into(),
        name: "总公司".into(),
        parent_id: Some(0),
        ancestors: "0".into(),
        sort: 0,
        status: "1".into(),
        remark: None,
        del_flag: dept::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let active: dept::ActiveModel = dept_model.into();
    dept::Entity::insert(active).exec(db).await.unwrap();

    // 创建管理员用户 (密码: test123)
    let password_hash = ryframe_auth::password::hash("test123").unwrap();
    let user_model = user::Model {
        id: 1,
        tenant_id: "system".into(),
        username: "admin".into(),
        password_hash,
        nickname: "管理员".into(),
        email: "admin@test.com".into(),
        phone: "13800000000".into(),
        avatar: None,
        status: user::Model::STATUS_NORMAL.to_string(),
        auth_version: 1,
        dept_id: Some(1),
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let active: user::ActiveModel = user_model.into();
    user::Entity::insert(active).exec(db).await.unwrap();

    // 创建 admin 角色
    let role_model = role::Model {
        id: 1,
        tenant_id: "system".into(),
        name: "超级管理员".into(),
        code: "admin".into(),
        is_super: 1,
        data_scope: "1".into(),
        status: "1".into(),
        sort: 0,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let active: role::ActiveModel = role_model.into();
    role::Entity::insert(active).exec(db).await.unwrap();

    let all_permission = permission::Model {
        id: 1,
        tenant_id: "system".into(),
        name: "全部权限".into(),
        code: "*:*:*".into(),
        parent_id: None,
        perm_type: "api".into(),
        icon: None,
        sort: 0,
        status: "1".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let active: permission::ActiveModel = all_permission.into();
    permission::Entity::insert(active).exec(db).await.unwrap();

    // 分配角色
    let user_role_model = ryframe_db::entities::user_role::Model {
        tenant_id: "system".into(),
        user_id: 1,
        role_id: 1,
    };
    let active: ryframe_db::entities::user_role::ActiveModel = user_role_model.into();
    ryframe_db::entities::user_role::Entity::insert(active)
        .exec(db)
        .await
        .unwrap();

    let role_permission_model = role_permission::Model {
        tenant_id: "system".into(),
        role_id: 1,
        perm_id: 1,
    };
    let active: role_permission::ActiveModel = role_permission_model.into();
    role_permission::Entity::insert(active)
        .exec(db)
        .await
        .unwrap();

    // 创建默认配置：关闭验证码（测试环境）
    let captcha_config = config::Model {
        tenant_id: "system".into(),
        id: 3,
        name: "账户验证码开关".into(),
        key: "sys.account.captchaEnabled".into(),
        value: "false".into(),
        remark: None,
        del_flag: config::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let active: config::ActiveModel = captcha_config.into();
    config::Entity::insert(active).exec(db).await.unwrap();
}

fn test_config() -> AppConfig {
    AppConfig {
        app: AppSettings {
            name: "test".into(),
            port: 0,
            ..Default::default()
        },
        database: DatabaseConfig {
            primary: DbConnection {
                database: "ryframe_api_integration".into(),
                max_connections: 5,
                ..Default::default()
            },
            ..Default::default()
        },
        generator: Default::default(),
        auth: AuthConfig {
            jwt_secret: "test-jwt-secret-for-integration-tests".into(),
            ..Default::default()
        },
        redis: None,
        logger: LoggerConfig {
            level: "warn".into(),
            ..Default::default()
        },
        rate_limit: RateLimitConfig::default(),
        cors: Default::default(),
        object_storage: Default::default(),
        proxy: Default::default(),
        upload: Default::default(),
    }
}

fn test_rate_limit_state() -> RateLimitState {
    RateLimitState {
        limiter: Arc::new(ryframe_middleware::RateLimiter::new_in_memory(100, 10)),
        config: Arc::new(RateLimitConfig::default()),
        trusted_proxies: Default::default(),
    }
}

async fn build_test_app(db: DatabaseConnection) -> AppState {
    build_test_app_with_redis(db, None).await
}

async fn build_test_app_with_redis(
    db: DatabaseConnection,
    redis: Option<ryframe_core::RedisClient>,
) -> AppState {
    let config = test_config();
    let config_arc = Arc::new(config.clone());
    let token_blacklist = ryframe_core::TokenBlacklist::new(redis.clone());
    let database = DatabaseCluster::single(db.clone());
    let auth_service = Arc::new(AuthService::new(
        database.clone(),
        config_arc.clone(),
        redis.clone(),
    ));
    let online_user = redis
        .as_ref()
        .map(|client| OnlineUserService::new_redis(client.clone()))
        .unwrap_or_default();
    AppState {
        auth: ryframe_auth::middleware::AuthState {
            config: config_arc.clone(),
            blacklist: token_blacklist.clone(),
            refresh_sessions: auth_service.refresh_sessions(),
            principal_resolver: auth_service.clone(),
        },
        monitor: ryframe_monitor::MonitorState {
            database: Arc::new(ryframe_db::SeaOrmDatabaseMonitor::new(database.clone())),
            redis: redis.clone(),
        },
        config: config_arc,
        services: Arc::new(AppServices {
            auth: auth_service,
            user: Arc::new(UserService::new(database.clone(), None)),
            role: Arc::new(RoleService::new(database.clone(), None)),
            tenant: Arc::new(TenantService::new(database.clone())),
            permission: Arc::new(PermissionService::new(database.clone(), None)),
            menu: Arc::new(MenuService::new(database.clone(), None)),
            dept: Arc::new(DeptService::new(database.clone(), None)),
            post: Arc::new(PostService::new(database.clone())),
            config: Arc::new(ConfigService::new(database.clone(), None)),
            dict: Arc::new(DictService::new(database.clone(), None)),
            notice: Arc::new(NoticeService::new(database.clone())),
            oper_log: Arc::new(OperLogService::new(database.clone())),
            login_info: Arc::new(LoginInfoService::new(database.clone())),
            generator: Arc::new(GeneratorService::new(
                database.clone(),
                "primary".into(),
                std::env::current_dir().unwrap(),
            )),
            profile: Arc::new(ProfileService::new(database.clone())),
            file: Arc::new(FileService::new(
                database,
                Arc::new(ryframe_storage::LocalObjectStorage::new("uploads")),
            )),
            online_user: Arc::new(online_user),
            captcha: CaptchaStore::new_in_memory(300),
        }),
        redis: redis.clone(),
        token_blacklist,
        rate_limiter: Arc::new(ryframe_middleware::RateLimiter::new_in_memory(100, 10)),
        trusted_proxies: Default::default(),
        runtime: RuntimeComponents::new(redis),
    }
}

/// 辅助：发送请求并返回 (StatusCode, Body JSON)
async fn send_request(
    app: axum::Router,
    mut req: Request<Body>,
) -> (StatusCode, serde_json::Value) {
    inject_unbound_csrf(&mut req);
    // Axum 0.8: oneshot() 不自动注入 ConnectInfo，需手动 mock
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:8080".parse::<SocketAddr>().unwrap()));
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    (status, json)
}

async fn send_request_with_headers(
    app: axum::Router,
    mut req: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    inject_unbound_csrf(&mut req);
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:8080".parse::<SocketAddr>().unwrap()));
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or_default();
    (status, headers, json)
}

fn inject_unbound_csrf(req: &mut Request<Body>) {
    if req.uri().path() != "/auth/login" || req.headers().contains_key("x-csrf-token") {
        return;
    }
    let token =
        ryframe_auth::jwt::encode_csrf("test-jwt-secret-for-integration-tests", None, 300).unwrap();
    req.headers_mut()
        .insert("x-csrf-token", token.parse().unwrap());
    req.headers_mut().insert(
        axum::http::header::COOKIE,
        format!("ryframe_csrf={token}").parse().unwrap(),
    );
}

fn response_cookie(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .filter_map(|value| value.split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.to_owned()))
}

fn response_cookie_header(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(&format!("{name}=")))
        .map(str::to_owned)
}

fn cookie_attribute<'a>(set_cookie: &'a str, name: &str) -> Option<&'a str> {
    set_cookie.split(';').skip(1).find_map(|attribute| {
        let (key, value) = attribute.trim().split_once('=')?;
        key.eq_ignore_ascii_case(name).then_some(value)
    })
}

// ==================== 测试用例 ====================

#[tokio::test]
async fn test_health_check() {
    let db = setup_test_db().await;
    let state = build_test_app(db.connection().clone()).await;
    let router = api_router(state, test_rate_limit_state());

    let req = Request::builder()
        .uri("/auth/login")
        .method("OPTIONS")
        .body(Body::empty())
        .unwrap();
    let _ = router.oneshot(req).await;
}

#[tokio::test]
async fn test_csrf_challenge_is_no_store_and_cookie_backed() {
    let db = setup_test_db().await;
    let state = build_test_app(db.connection().clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let request = Request::builder()
        .uri("/auth/csrf")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = send_request_with_headers(router, request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("cache-control").unwrap(), "no-store");
    let csrf = body["data"]["csrf_token"].as_str().unwrap();
    assert_eq!(body["data"]["expires_in"], 300);
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("ryframe_csrf="))
        .unwrap();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("Expires="));
    assert!(cookie.contains("Max-Age=300"));
    assert!(cookie.contains("SameSite=Lax"));
    assert!(cookie.contains("Path=/api/v1/auth"));
    assert_eq!(
        response_cookie(&headers, "ryframe_csrf").as_deref(),
        Some(csrf)
    );
}

#[tokio::test]
async fn test_login_rejects_missing_csrf_cookie() {
    let db = setup_test_db().await;
    let state = build_test_app(db.connection().clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let request = Request::builder()
        .uri("/auth/login")
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        // Suppress the test helper's automatic challenge while deliberately
        // omitting the matching challenge cookie.
        .header("x-csrf-token", "invalid")
        .body(Body::from(r#"{"username":"admin","password":"test123"}"#))
        .unwrap();
    let (status, _) = send_request(router, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_csrf_challenge_rejects_unconfigured_origin() {
    let db = setup_test_db().await;
    let state = build_test_app(db.connection().clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let request = Request::builder()
        .uri("/auth/csrf")
        .method("GET")
        .header("origin", "https://evil.example")
        .body(Body::empty())
        .unwrap();
    let (status, headers, _) = send_request_with_headers(router, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(headers.get_all("set-cookie").iter().count(), 0);
}

#[tokio::test]
async fn test_login_is_limited_to_five_attempts_per_minute() {
    let db = setup_test_db().await;
    let state = build_test_app(db.connection().clone()).await;
    let router = api_router(state, test_rate_limit_state());

    for attempt in 1..=6 {
        let request = Request::builder()
            .uri("/auth/login")
            .method("POST")
            .header("X-Tenant-Id", "system")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"username":"rate-limited-user","password":"invalid"}"#,
            ))
            .unwrap();
        let (status, headers, _) = send_request_with_headers(router.clone(), request).await;
        if attempt <= 5 {
            assert_ne!(status, StatusCode::TOO_MANY_REQUESTS);
        } else {
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
            assert!(headers.get("retry-after").is_some());
        }
    }
}

/// 认证全流程：登录 → me → 刷新 → 错误场景
#[tokio::test]
async fn test_auth_full_flow() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;

    // 1. 登录成功
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri("/auth/login")
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "username": "admin",
                "password": "test123"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (status, login_headers, body) = send_request_with_headers(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let access_token = body["data"]["access_token"].as_str().unwrap().to_string();
    assert!(body["data"].get("refresh_token").is_none());
    let refresh_token = response_cookie(&login_headers, "ryframe_refresh_token").unwrap();
    let refresh_cookie_header =
        response_cookie_header(&login_headers, "ryframe_refresh_token").unwrap();
    assert!(refresh_cookie_header.contains("HttpOnly"));
    assert!(refresh_cookie_header.contains("Expires="));
    assert!(refresh_cookie_header.contains("Max-Age="));
    assert!(refresh_cookie_header.contains("SameSite=Lax"));
    assert!(refresh_cookie_header.contains("Path=/api/v1/auth"));
    assert!(!refresh_cookie_header.contains("Domain="));
    assert!(!access_token.is_empty());
    assert_eq!(body["data"]["user_info"]["username"], "admin");
    assert_eq!(
        body["data"]["user_info"]["roles"],
        serde_json::json!(["admin"])
    );
    assert_eq!(
        body["data"]["user_info"]["perms"],
        serde_json::json!(["*:*:*"])
    );

    // 2. 用 token 访问 /auth/me
    let state2 = build_test_app(db.clone()).await;
    let router2 = api_router(state2, test_rate_limit_state());
    let me_req = Request::builder()
        .uri("/auth/me")
        .method("GET")
        .header("authorization", format!("Bearer {}", access_token))
        .body(Body::empty())
        .unwrap();
    let (s2, b2) = send_request(router2, me_req).await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(b2["data"]["username"], "admin");
    assert_eq!(b2["data"]["roles"], body["data"]["user_info"]["roles"]);
    assert_eq!(b2["data"]["perms"], body["data"]["user_info"]["perms"]);

    // 3. 刷新令牌
    let state3 = build_test_app(db.clone()).await;
    let router3 = api_router(state3, test_rate_limit_state());
    let refresh_claims =
        ryframe_auth::jwt::decode_token(&refresh_token, "test-jwt-secret-for-integration-tests")
            .unwrap();
    let csrf = ryframe_auth::jwt::encode_csrf(
        "test-jwt-secret-for-integration-tests",
        Some(&refresh_claims.sid),
        300,
    )
    .unwrap();
    let missing_csrf = Request::builder()
        .uri("/auth/refresh")
        .method("POST")
        .header("cookie", format!("ryframe_refresh_token={refresh_token}"))
        .body(Body::empty())
        .unwrap();
    let (status, headers, _) = send_request_with_headers(router3.clone(), missing_csrf).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(headers.get_all("set-cookie").iter().count(), 0);

    // Ensure a sliding seven-day cookie would move its Expires timestamp.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    let refresh_req = Request::builder()
        .uri("/auth/refresh")
        .method("POST")
        .header("x-csrf-token", &csrf)
        .header(
            "cookie",
            format!("ryframe_refresh_token={refresh_token}; ryframe_csrf={csrf}"),
        )
        .body(Body::empty())
        .unwrap();
    let (s3, refresh_headers, b3) = send_request_with_headers(router3.clone(), refresh_req).await;
    assert_eq!(s3, StatusCode::OK);
    let committed_refresh = response_cookie(&refresh_headers, "ryframe_refresh_token").unwrap();
    let rotated_cookie_header =
        response_cookie_header(&refresh_headers, "ryframe_refresh_token").unwrap();
    let login_expires = cookie_attribute(&refresh_cookie_header, "Expires").unwrap();
    let rotated_expires = cookie_attribute(&rotated_cookie_header, "Expires").unwrap();
    assert_eq!(
        rotated_expires, login_expires,
        "refresh rotation must preserve the login-time absolute expiry"
    );
    let login_max_age: i64 = cookie_attribute(&refresh_cookie_header, "Max-Age")
        .unwrap()
        .parse()
        .unwrap();
    let rotated_max_age: i64 = cookie_attribute(&rotated_cookie_header, "Max-Age")
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        rotated_max_age < login_max_age,
        "refresh Max-Age must count down instead of resetting to seven days"
    );
    let committed_claims = ryframe_auth::jwt::decode_token(
        &committed_refresh,
        "test-jwt-secret-for-integration-tests",
    )
    .unwrap();
    assert_eq!(committed_claims.exp, refresh_claims.exp);
    assert!(b3["data"].get("access_token").is_some());
    assert_eq!(
        b3["data"]["user_info"]["roles"],
        body["data"]["user_info"]["roles"]
    );
    assert_eq!(
        b3["data"]["user_info"]["perms"],
        body["data"]["user_info"]["perms"]
    );

    // If the success response is lost, retrying the old cookie with the same
    // signed CSRF challenge recovers the exact committed refresh token.
    let recovered_req = Request::builder()
        .uri("/auth/refresh")
        .method("POST")
        .header("x-csrf-token", &csrf)
        .header(
            "cookie",
            format!("ryframe_refresh_token={refresh_token}; ryframe_csrf={csrf}"),
        )
        .body(Body::empty())
        .unwrap();
    let (recovered_status, recovered_headers, _) =
        send_request_with_headers(router3.clone(), recovered_req).await;
    assert_eq!(recovered_status, StatusCode::OK);
    assert_eq!(
        response_cookie(&recovered_headers, "ryframe_refresh_token").as_deref(),
        Some(committed_refresh.as_str())
    );

    let competing_csrf = ryframe_auth::jwt::encode_csrf(
        "test-jwt-secret-for-integration-tests",
        Some(&refresh_claims.sid),
        300,
    )
    .unwrap();
    let competing_req = Request::builder()
        .uri("/auth/refresh")
        .method("POST")
        .header("x-csrf-token", &competing_csrf)
        .header(
            "cookie",
            format!("ryframe_refresh_token={refresh_token}; ryframe_csrf={competing_csrf}"),
        )
        .body(Body::empty())
        .unwrap();
    let (competing_status, competing_headers, _) =
        send_request_with_headers(router3, competing_req).await;
    assert_eq!(competing_status, StatusCode::CONFLICT);
    assert_eq!(competing_headers.get("retry-after").unwrap(), "5");

    // 4. 错误密码
    let state4 = build_test_app(db.clone()).await;
    let router4 = api_router(state4, test_rate_limit_state());
    let bad_req = Request::builder()
        .uri("/auth/login")
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "username": "admin",
                "password": "wrong"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (s4, _) = send_request(router4, bad_req).await;
    assert_eq!(s4, StatusCode::UNAUTHORIZED);

    // 5. 用户不存在
    let state5 = build_test_app(db.clone()).await;
    let router5 = api_router(state5, test_rate_limit_state());
    let notfound_req = Request::builder()
        .uri("/auth/login")
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "username": "nonexistent",
                "password": "test123"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (s5, _) = send_request(router5, notfound_req).await;
    assert_eq!(s5, StatusCode::UNAUTHORIZED);

    // 6. 无 token 访问 /auth/me
    let state6 = build_test_app(db.connection().clone()).await;
    let router6 = api_router(state6, test_rate_limit_state());
    let noauth_req = Request::builder()
        .uri("/auth/me")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let (s6, _) = send_request(router6, noauth_req).await;
    assert_eq!(s6, StatusCode::UNAUTHORIZED);
}

// ==================== 系统管理集成测试 ====================

/// 辅助：登录并返回 access_token
async fn login_get_token(db: &DatabaseConnection) -> String {
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri("/auth/login")
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "username": "admin",
                "password": "test123"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (status, body) = send_request(router, req).await;
    assert_eq!(status, StatusCode::OK);
    body["data"]["access_token"].as_str().unwrap().to_string()
}

/// 辅助：发送带认证的 GET 请求
async fn auth_get(
    db: &DatabaseConnection,
    uri: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri(uri)
        .method("GET")
        .header("X-Tenant-Id", "system")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    send_request(router, req).await
}

/// 辅助：发送带认证的 POST 请求
async fn auth_post(
    db: &DatabaseConnection,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri(uri)
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    send_request(router, req).await
}

/// 辅助：发送带认证的 PUT 请求
async fn auth_put(
    db: &DatabaseConnection,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri(uri)
        .method("PUT")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    send_request(router, req).await
}

/// 辅助：发送带认证的 DELETE 请求
async fn auth_delete(
    db: &DatabaseConnection,
    uri: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri(uri)
        .method("DELETE")
        .header("X-Tenant-Id", "system")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    send_request(router, req).await
}

/// 系统 CRUD 全流程：岗位/配置/字典/通知的创建 + 查询
#[tokio::test]
async fn test_system_crud_operations() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 岗位 CRUD
    let (s, b) = auth_post(
        &db,
        "/system/posts",
        &token,
        serde_json::json!({
            "name": "测试岗位", "code": "test_post", "sort": 1
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["name"], "测试岗位");
    let (s, _) = auth_get(&db, "/system/posts?page=1&page_size=10", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, b) = auth_get(
        &db,
        "/system/posts?page=1&page_size=10&code=test_post",
        &token,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["total"], 1);

    let (s, b) = auth_get(&db, "/system/posts/all?code=test_post", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"].as_array().unwrap().len(), 1);

    // 配置 CRUD
    let (s, b) = auth_post(
        &db,
        "/system/configs",
        &token,
        serde_json::json!({
            "name": "测试参数", "key": "test.config.key", "value": "test_value"
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["key"], "test.config.key");
    let (s, b) = auth_get(
        &db,
        "/system/configs?page=1&page_size=10&key=test.config.key",
        &token,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["total"], 1);
    assert_eq!(b["rows"][0]["key"], "test.config.key");

    let (s, _) = auth_get(&db, "/system/configs/all", &token).await;
    assert_eq!(s, StatusCode::OK);

    // 字典 CRUD
    let (s, b) = auth_post(
        &db,
        "/system/dict/types",
        &token,
        serde_json::json!({
            "name": "测试字典", "code": "test_dict"
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["code"], "test_dict");
    let (s, b) = auth_get(
        &db,
        "/system/dict/types?page=1&page_size=10&code=test_dict",
        &token,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["total"], 1);

    let (s, b) = auth_get(&db, "/system/dict/types/all?code=test_dict", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"].as_array().unwrap().len(), 1);

    // 通知 CRUD
    let (s, b) = auth_post(
        &db,
        "/system/notices",
        &token,
        serde_json::json!({
            "title": "测试公告", "content": "这是一条测试公告"
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["title"], "测试公告");
    let (s, b) = auth_get(&db, "/system/notices?page=1&page_size=10&status=1", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["total"], 1);

    let (s, b) = auth_get(&db, "/system/notices/all?status=1", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"].as_array().unwrap().len(), 1);
}

/// 系统查询接口：用户/角色/部门/菜单/权限/在线用户
#[tokio::test]
async fn test_system_query_endpoints() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    let (s, b) = auth_get(&db, "/system/users?page=1&page_size=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some());

    let (s, _) = auth_get(&db, "/system/users/all", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, b) = auth_get(&db, "/system/roles?page=1&page_size=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some());

    let (s, b) = auth_get(&db, "/system/roles/all?code=admin", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"].as_array().unwrap().len(), 1);

    let (s, b) = auth_get(&db, "/system/depts/tree", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["data"].as_array().is_some());

    let (s, b) = auth_get(&db, "/system/depts?page=1&page_size=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some());

    let (s, _) = auth_get(&db, "/system/depts/all", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, b) = auth_get(&db, "/system/menus/tree", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["data"].as_array().is_some());

    let (s, b) = auth_get(&db, "/system/menus?page=1&page_size=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some());

    let (s, _) = auth_get(&db, "/system/menus/all", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, b) = auth_get(&db, "/system/perms/tree", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["data"].as_array().is_some());

    let (s, b) = auth_get(&db, "/system/online", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["rows"].as_array().is_some());
}

/// 未认证访问系统接口应返回 401
#[tokio::test]
async fn test_unauthenticated_access_denied() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;

    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let endpoints = vec![
        "/system/users?page=1&page_size=10",
        "/system/roles?page=1&page_size=10",
        "/system/depts/tree",
        "/system/depts?page=1&page_size=10",
        "/system/posts?page=1&page_size=10",
        "/system/configs?page=1&page_size=10",
    ];
    for uri in endpoints {
        let req = Request::builder()
            .uri(uri)
            .method("GET")
            .body(Body::empty())
            .unwrap();
        let (status, _) = send_request(router.clone(), req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "访问 {} 应返回 401", uri);
    }
}

// ==================== PUT/DELETE 全流程测试 ====================

/// PUT/DELETE 操作全流程：所有实体的更新和删除
#[tokio::test]
async fn test_update_and_delete_operations() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // ===== 岗位：创建 → 更新 → 删除 =====
    let (s, b) = auth_post(
        &db,
        "/system/posts",
        &token,
        serde_json::json!({"name": "临时岗位", "code": "temp_post", "sort": 99}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let post_id = b["data"]["id"].as_str().unwrap().parse::<i64>().unwrap();

    let (s, _) = auth_put(
        &db,
        &format!("/system/posts/{}", post_id),
        &token,
        serde_json::json!({"name": "更新后的岗位", "sort": 10, "status": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_delete(&db, &format!("/system/posts/{}", post_id), &token).await;
    assert_eq!(s, StatusCode::OK);

    // ===== 配置：创建 → 更新 → 通过 key 查询 → 删除 =====
    let (s, b) = auth_post(
        &db,
        "/system/configs",
        &token,
        serde_json::json!({"name": "临时参数", "key": "temp.config", "value": "old_value"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let cfg_id = b["data"]["id"].as_str().unwrap().parse::<i64>().unwrap();

    let (s, _) = auth_put(
        &db,
        &format!("/system/configs/{}", cfg_id),
        &token,
        serde_json::json!({"value": "new_value"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 验证 config 可通过 key 查询到（更新后 key 仍存在）
    let (s, b) = auth_get(&db, "/system/configs/key/temp.config", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["data"].as_str().is_some(), "配置值应存在");

    let (s, _) = auth_delete(&db, "/system/configs/cache", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_delete(&db, &format!("/system/configs/{}", cfg_id), &token).await;
    assert_eq!(s, StatusCode::OK);

    // ===== 字典类型：创建 → 更新 → 创建数据 → 更新数据 → 删除数据 → 删除类型 =====
    let (s, b) = auth_post(
        &db,
        "/system/dict/types",
        &token,
        serde_json::json!({"name": "临时字典", "code": "temp_dict"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let dict_type_id = b["data"]["id"].as_str().unwrap().parse::<i64>().unwrap();

    let (s, _) = auth_put(
        &db,
        &format!("/system/dict/types/{}", dict_type_id),
        &token,
        serde_json::json!({"name": "改名字典", "status": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 创建字典数据
    let (s, b) = auth_post(
        &db,
        "/system/dict/data",
        &token,
        serde_json::json!({"type_code": "temp_dict", "label": "男", "value": "0", "sort": 0}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let dict_data_id = b["data"]["id"].as_str().unwrap().parse::<i64>().unwrap();

    // 更新字典数据
    let (s, _) = auth_put(
        &db,
        &format!("/system/dict/data/{}", dict_data_id),
        &token,
        serde_json::json!({"label": "男性", "value": "0", "sort": 0, "status": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 通过 type_code 路径查询字典数据
    let (s, b) = auth_get(&db, "/system/dict/data/type/temp_dict", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!b["data"].as_array().unwrap().is_empty());

    // 删除字典数据
    let (s, _) = auth_delete(&db, &format!("/system/dict/data/{}", dict_data_id), &token).await;
    assert_eq!(s, StatusCode::OK);

    // 删除字典类型
    let (s, _) = auth_delete(&db, &format!("/system/dict/types/{}", dict_type_id), &token).await;
    assert_eq!(s, StatusCode::OK);

    // ===== 通知：创建 → 更新 → 删除 =====
    let (s, b) = auth_post(
        &db,
        "/system/notices",
        &token,
        serde_json::json!({"title": "旧标题", "content": "旧内容", "notice_type": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let notice_id = b["data"]["id"].as_str().unwrap().parse::<i64>().unwrap();

    let (s, _) = auth_put(
        &db,
        &format!("/system/notices/{}", notice_id),
        &token,
        serde_json::json!({"title": "新标题", "content": "新内容", "notice_type": "2", "status": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_delete(&db, &format!("/system/notices/{}", notice_id), &token).await;
    assert_eq!(s, StatusCode::OK);

    // ===== 部门：创建 → 更新 → 删除 =====
    let (s, b) = auth_post(
        &db,
        "/system/depts",
        &token,
        serde_json::json!({"name": "子部门", "parent_id": "1", "sort": 0}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let dept_id = b["data"]["id"].as_str().unwrap();

    let (s, _) = auth_put(
        &db,
        &format!("/system/depts/{}", dept_id),
        &token,
        serde_json::json!({"name": "改名部门", "parent_id": "1", "sort": 1, "status": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_delete(&db, &format!("/system/depts/{}", dept_id), &token).await;
    assert_eq!(s, StatusCode::OK);

    // ===== 菜单：创建 → 更新 → 删除 =====
    let (s, b) = auth_post(
        &db,
        "/system/menus",
        &token,
        serde_json::json!({"name": "测试菜单", "parent_id": null, "menu_type": "C", "perm_id": "1", "route_key": "test.menu", "icon": null, "sort": 0, "visible": true}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let menu_id = b["data"]["id"].as_str().unwrap();

    let (s, _) = auth_put(
        &db,
        &format!("/system/menus/{}", menu_id),
        &token,
        serde_json::json!({"name": "改名菜单", "parent_id": null, "menu_type": "C", "perm_id": "1", "route_key": "test.menu", "icon": null, "sort": 1, "visible": true, "status": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 旧菜单底层字段不再被接受
    let (s, _) = auth_post(
        &db,
        "/system/menus",
        &token,
        serde_json::json!({"name": "旧字段菜单", "parent_id": null, "menu_type": "C", "path": "/legacy", "icon": null, "sort": 0, "visible": true}),
    )
    .await;
    assert!(
        !s.is_success(),
        "菜单创建请求不应再接受 path/component/perms 等旧字段"
    );

    let (s, _) = auth_post(
        &db,
        "/system/menus",
        &token,
        serde_json::json!({"name": "非法菜单", "parent_id": null, "menu_type": "X", "perm_id": null, "route_key": null, "icon": null, "sort": 0, "visible": true}),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    let (s, _) = auth_delete(&db, &format!("/system/menus/{}", menu_id), &token).await;
    assert_eq!(s, StatusCode::OK);

    // ===== 角色：创建 → 更新 → 分配权限 → 删除 =====
    let (s, b) = auth_post(
        &db,
        "/system/roles",
        &token,
        serde_json::json!({"name": "临时角色", "code": "temp_role", "sort": 5, "data_scope": "5"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let role_id = b["data"]["id"].as_str().unwrap().parse::<i64>().unwrap();

    let (s, _) = auth_put(
        &db,
        &format!("/system/roles/{}", role_id),
        &token,
        serde_json::json!({"name": "改名角色", "sort": 3, "status": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_put(
        &db,
        "/auth/profile/password",
        &token,
        serde_json::json!({"old_password": "test123", "new_password": "newpass456"}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, _) = auth_put(
        &db,
        &format!("/system/roles/{}/data-scope", role_id),
        &token,
        serde_json::json!({"data_scope": "2", "dept_ids": ["1"]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = auth_put(
        &db,
        &format!("/system/roles/{}/data-scope", role_id),
        &token,
        serde_json::json!({"data_scope": "9", "dept_ids": []}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // 分配权限
    let (s, _) = auth_put(
        &db,
        &format!("/system/roles/{}/permissions", role_id),
        &token,
        serde_json::json!({"perm_ids": []}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 旧角色菜单分配接口不再注册
    let (s, _) = auth_put(
        &db,
        &format!("/system/roles/{}/menus", role_id),
        &token,
        serde_json::json!({"menu_ids": []}),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // 切换为非自定义范围时原子清空部门关联
    let (s, _) = auth_put(
        &db,
        &format!("/system/roles/{}/data-scope", role_id),
        &token,
        serde_json::json!({"data_scope": "5", "dept_ids": []}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_delete(&db, &format!("/system/roles/{}", role_id), &token).await;
    assert_eq!(s, StatusCode::OK);

    // ===== 用户：创建 → 更新 → 修改状态 → 重置密码 → 删除 =====
    let (s, b) = auth_post(
        &db, "/system/users", &token,
        serde_json::json!({"username": "testupdate", "nickname": "测试更新", "email": null, "phone": null, "dept_id": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let user_id = b["data"]["id"].as_str().unwrap().parse::<i64>().unwrap();
    assert_eq!(b["data"]["status"], "pending_activation");

    // 旧创建用户密码字段不再被接受
    let (s, _) = auth_post(
        &db,
        "/system/users",
        &token,
        serde_json::json!({"username": "legacy_password_user", "nickname": "旧密码字段", "password": "newpass123", "email": null, "phone": null, "dept_id": "1"}),
    )
    .await;
    assert!(!s.is_success(), "创建用户请求不应再接受 password 字段");

    // 旧创建用户 sex 字段不再被接受
    let (s, _) = auth_post(
        &db,
        "/system/users",
        &token,
        serde_json::json!({"username": "legacy_sex_user", "nickname": "旧性别字段", "sex": "0", "email": null, "phone": null, "dept_id": "1"}),
    )
    .await;
    assert!(!s.is_success(), "创建用户请求不应再接受 sex 字段");

    // 更新用户
    let (s, _) = auth_put(
        &db,
        &format!("/system/users/{}", user_id),
        &token,
        serde_json::json!({"nickname": "已更新", "email": null, "phone": null, "dept_id": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 修改用户状态
    let (s, _) = auth_put(
        &db,
        &format!("/system/users/{}/status", user_id),
        &token,
        serde_json::json!({"status": "0"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 旧管理员重置密码接口不可用
    let (s, _) = auth_put(
        &db,
        &format!("/system/users/{}/password", user_id),
        &token,
        serde_json::json!({"password": "newpass123"}),
    )
    .await;
    assert!(!s.is_success());

    // 发起密码重置请求
    let (s, body) = auth_post(
        &db,
        &format!("/system/users/{}/password-reset-requests", user_id),
        &token,
        serde_json::json!({"reason": "用户忘记密码"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let reset_data = &body["data"];
    assert!(reset_data["request_id"].as_str().is_some());
    assert!(reset_data["reset_token"].as_str().is_some());
    assert!(reset_data["reset_url"].as_str().is_some());

    let request_id = reset_data["request_id"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let request_row = ryframe_db::entities::password_reset_request::Entity::find_by_id(request_id)
        .one(&db)
        .await
        .unwrap()
        .expect("密码重置请求应存在");
    assert_eq!(request_row.target_user_id, user_id);
    assert_eq!(request_row.tenant_id, "system");

    let target_via_exact_filter = user::Entity::find_by_id(user_id)
        .filter(user::Column::TenantId.eq("system"))
        .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
        .one(&db)
        .await
        .unwrap();
    assert!(
        target_via_exact_filter.is_some(),
        "密码重置前按 id+tenant+del_flag 精确查询应能找到用户"
    );

    let target_before_reset = user::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .expect("密码重置前目标用户应存在");
    assert_eq!(target_before_reset.del_flag, user::Model::DEL_FLAG_NORMAL);

    // 用户通过公开链接完成密码重置
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri("/auth/password-reset/complete")
        .method("POST")
        .header("content-type", "application/json")
        .header("X-Tenant-Id", "system")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "tenant_id": "system",
                "request_id": reset_data["request_id"].as_str().unwrap(),
                "token": reset_data["reset_token"].as_str().unwrap(),
                "new_password": "NewPass123!"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (s, body) = send_request(router, req).await;
    assert_eq!(s, StatusCode::OK, "完成密码重置失败: {body}");

    // 删除用户
    let (s, body) = auth_delete(&db, &format!("/system/users/{}", user_id), &token).await;
    assert_eq!(s, StatusCode::OK, "删除用户失败: {body}");
}

/// 404 错误场景：访问不存在的资源
#[tokio::test]
async fn test_not_found_error_scenarios() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 不存在资源的 detail 端点（GET by id）
    let not_found_endpoints = vec![
        "/system/users/99999",
        "/system/roles/99999",
        "/system/depts/99999",
        "/system/menus/99999",
        "/system/posts/99999",
        "/system/configs/99999",
        "/system/notices/99999",
    ];

    for uri in &not_found_endpoints {
        let (status, _) = auth_get(&db, uri, &token).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {} 应返回 404", uri);
    }

    // dict/types/{id} 没有 GET detail 路由，返回 405
    let (s, _) = auth_get(&db, "/system/dict/types/99999", &token).await;
    assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED);

    // dict/data/{id} 没有 GET detail 路由，返回 405
    let (s, _) = auth_get(&db, "/system/dict/data/99999", &token).await;
    assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED);

    // 不存在的配置 key
    let (s, _) = auth_get(&db, "/system/configs/key/nonexistent_key", &token).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // 不存在的字典类型数据查询 — 返回 200 但 data 为空数组
    let (s, b) = auth_get(&db, "/system/dict/data/type/nonexistent_type", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        b["data"].as_array().map(|a| a.is_empty()).unwrap_or(false),
        "不存在的类型应返回空数组"
    );
}

/// 重复键冲突场景
#[tokio::test]
async fn test_duplicate_key_conflicts() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 创建已有的 post code
    let _ = auth_post(
        &db,
        "/system/posts",
        &token,
        serde_json::json!({"name": "唯一岗位", "code": "unique_post", "sort": 1}),
    )
    .await;

    // 尝试用相同 code 创建另一个 post
    let (s, _) = auth_post(
        &db,
        "/system/posts",
        &token,
        serde_json::json!({"name": "重复岗位", "code": "unique_post", "sort": 2}),
    )
    .await;
    assert!(!s.is_success(), "重复岗位编码应返回错误");

    // 创建已有的 config key
    let _ = auth_post(
        &db,
        "/system/configs",
        &token,
        serde_json::json!({"name": "唯一配置", "key": "unique.config", "value": "val"}),
    )
    .await;

    // 尝试用相同 key 创建另一个 config
    let (s, _) = auth_post(
        &db,
        "/system/configs",
        &token,
        serde_json::json!({"name": "重复配置", "key": "unique.config", "value": "other"}),
    )
    .await;
    assert!(!s.is_success(), "重复配置键应返回错误");

    // 创建已有的 role code
    let _ = auth_post(
        &db,
        "/system/roles",
        &token,
        serde_json::json!({"name": "唯一角色", "code": "unique_role", "sort": 1, "data_scope": "1"}),
    )
    .await;

    // 尝试用相同 code 创建另一个 role
    let (s, _) = auth_post(
        &db,
        "/system/roles",
        &token,
        serde_json::json!({"name": "重复角色", "code": "unique_role", "sort": 2, "data_scope": "1"}),
    )
    .await;
    assert!(!s.is_success(), "重复角色编码应返回错误");

    // 创建已有的 username
    let _ = auth_post(
        &db, "/system/users", &token,
        serde_json::json!({"username": "duplicate_user", "nickname": "重复用户", "email": null, "phone": null, "dept_id": "1"}),
    )
    .await;

    // 尝试用相同 username 创建另一个 user
    let (s, _) = auth_post(
        &db, "/system/users", &token,
        serde_json::json!({"username": "duplicate_user", "nickname": "重名用户", "email": null, "phone": null, "dept_id": "1"}),
    )
    .await;
    assert!(!s.is_success(), "重复用户名应返回错误");
}

/// 参数校验错误场景
#[tokio::test]
async fn test_validation_error_scenarios() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 空名称创建岗位
    let (s, _) = auth_post(
        &db,
        "/system/posts",
        &token,
        serde_json::json!({"name": "", "code": "empty_name", "sort": 1}),
    )
    .await;
    assert!(!s.is_success(), "空岗位名称应返回校验错误");

    // 空编码创建岗位
    let (s, _) = auth_post(
        &db,
        "/system/posts",
        &token,
        serde_json::json!({"name": "有效名称", "code": "", "sort": 1}),
    )
    .await;
    assert!(!s.is_success(), "空岗位编码应返回校验错误");

    // 空 key 创建配置
    let (s, _) = auth_post(
        &db,
        "/system/configs",
        &token,
        serde_json::json!({"name": "空键配置", "key": "", "value": "val"}),
    )
    .await;
    assert!(!s.is_success(), "空配置键应返回校验错误");

    // 空 value 创建配置
    let (s, _) = auth_post(
        &db,
        "/system/configs",
        &token,
        serde_json::json!({"name": "空值配置", "key": "empty.val", "value": ""}),
    )
    .await;
    assert!(!s.is_success(), "空配置值应返回校验错误");

    // 空标题创建通知
    let (s, _) = auth_post(
        &db,
        "/system/notices",
        &token,
        serde_json::json!({"title": "", "content": "内容内容"}),
    )
    .await;
    assert!(!s.is_success(), "空标题应返回校验错误");

    // 空内容创建通知
    let (s, _) = auth_post(
        &db,
        "/system/notices",
        &token,
        serde_json::json!({"title": "有标题", "content": ""}),
    )
    .await;
    assert!(!s.is_success(), "空内容应返回校验错误");

    // 空名称创建字典类型
    let (s, _) = auth_post(
        &db,
        "/system/dict/types",
        &token,
        serde_json::json!({"name": "", "code": "empty_name"}),
    )
    .await;
    assert!(!s.is_success(), "空字典名称应返回校验错误");

    // 空编码创建字典类型
    let (s, _) = auth_post(
        &db,
        "/system/dict/types",
        &token,
        serde_json::json!({"name": "有名称", "code": ""}),
    )
    .await;
    assert!(!s.is_success(), "空字典编码应返回校验错误");
}

// ==================== 监控端点测试 ====================

/// 监控端点（/metrics 公开，/server /cache /db-pool 需认证；旧 health 已删除）
#[tokio::test]
async fn test_monitor_endpoints() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());

    // 服务器信息（需认证）
    let req = Request::builder()
        .uri("/monitor/server")
        .method("GET")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let (status, body) = send_request(router.clone(), req).await;
    assert_eq!(status, StatusCode::OK, "监控服务器信息应返回 200");
    assert!(body["data"].get("cpu_cores").is_some(), "应包含 CPU 核心数");

    // v0.5 删除旧监控 health；根路由改用 /livez 和 /readyz。
    let req = Request::builder()
        .uri("/monitor/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let (status, _) = send_request(router.clone(), req).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 缓存信息（需认证）
    let req = Request::builder()
        .uri("/monitor/cache")
        .method("GET")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send_request(router.clone(), req).await;
    assert_eq!(status, StatusCode::OK);

    // 数据库连接池（需认证）
    let req = Request::builder()
        .uri("/monitor/db-pool")
        .method("GET")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let (status, body) = send_request(router.clone(), req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], "connected");
}

// ==================== 日志端点测试 ====================

/// 登录日志/操作日志查询
#[tokio::test]
async fn test_log_endpoints() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 登录日志列表
    let (s, b) = auth_get(&db, "/system/loginlogs?page=1&page_size=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some(), "登录日志应返回 rows");

    // 登录日志不分页
    let (s, _) = auth_get(&db, "/system/loginlogs/all", &token).await;
    assert_eq!(s, StatusCode::OK);

    // 清空登录日志路由不再对业务管理端开放
    let (s, _) = auth_delete(&db, "/system/loginlogs/clean", &token).await;
    assert!(!s.is_success());

    // 操作日志列表
    let (s, b) = auth_get(&db, "/system/operlogs?page=1&page_size=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some(), "操作日志应返回 rows");

    // 操作日志不分页
    let (s, _) = auth_get(&db, "/system/operlogs/all", &token).await;
    assert_eq!(s, StatusCode::OK);

    // 清空操作日志路由不再对业务管理端开放
    let (s, _) = auth_delete(&db, "/system/operlogs/clean", &token).await;
    assert!(!s.is_success());
}

// ==================== 个人中心端点测试 ====================

/// 个人中心：获取/更新信息、修改密码
#[tokio::test]
async fn test_profile_endpoints() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    let (s, b) = auth_get(&db, "/auth/profile", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["username"], "admin");

    let (s, _) = auth_put(
        &db,
        "/auth/profile",
        &token,
        serde_json::json!({"nickname": "NewNick", "email": "new@test.com", "phone": "13900000000"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_put(
        &db,
        "/auth/profile/password",
        &token,
        serde_json::json!({"old_password": "test123", "new_password": "NewPass456!"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_get(&db, "/auth/profile", &token).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let saved_user = user::Entity::find()
        .filter(user::Column::Username.eq("admin"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved_user.auth_version, 2);
    assert!(ryframe_auth::password::verify("NewPass456!", &saved_user.password_hash).unwrap());
}

// ==================== 登出测试 ====================

fn force_logout_principal(tenant_id: &str) -> ryframe_auth::RequestPrincipal {
    ryframe_auth::RequestPrincipal {
        actor: ryframe_common::ActorContext {
            user_id: 1,
            tenant_id: tenant_id.into(),
            username: "admin".into(),
            dept_id: None,
            dept_path: None,
            data_scope: ryframe_common::DataScope::All,
            custom_dept_ids: Vec::new(),
            include_self: true,
            is_super_admin: true,
        },
        roles: vec!["admin".into()],
        role_ids: vec![1],
        permissions: vec!["monitor:online:force-logout".into()],
        tenant_request_limit_per_minute: 1_000,
    }
}

/// Exercises the handler orchestration against the Compose Redis service:
/// tenant validation, revoke-before-index-delete, transient 503, retry, and
/// idempotent repeated force logout.
#[tokio::test]
#[ignore = "requires Docker Compose MySQL and Redis services"]
async fn force_logout_uses_authoritative_family_and_recovers_after_redis_failure() {
    let db = setup_test_db().await;
    let port = std::env::var("RYFRAME_TEST_REDIS_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(16379);
    let redis = ryframe_core::RedisClient::connect(&ryframe_config::RedisConfig {
        port,
        database: 14,
        timeout_secs: 1,
        ..Default::default()
    })
    .await
    .expect("connect Docker Compose Redis service");
    let state = build_test_app_with_redis(db.connection().clone(), Some(redis.clone())).await;
    let sid = format!(
        "sid-force-handler-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_micros()
    );
    let absolute_exp = chrono::Utc::now().timestamp() + 600;
    let refresh_sessions = state.services.auth.refresh_sessions();
    refresh_sessions
        .register(ryframe_core::RefreshFamily {
            sid: sid.clone(),
            tenant_id: "system".into(),
            user_id: 1,
            current_jti: "force-handler-jti".into(),
            previous_jti: None,
            last_attempt_id: None,
            rotated_at: 0,
            absolute_exp,
            revoked: false,
        })
        .await
        .unwrap();
    let now = chrono::Utc::now();
    state
        .services
        .online_user
        .add_user(ryframe_service::system::UserSession {
            sid: sid.clone(),
            tenant_id: "system".into(),
            user_id: 1,
            username: "target".into(),
            dept_name: None,
            ipaddr: "127.0.0.1".into(),
            login_location: None,
            browser: None,
            os: None,
            login_time: now,
            last_access_time: now,
            absolute_exp,
        })
        .await;

    let system_actor = force_logout_principal("system");
    let cross_tenant = online_user_handler::force_logout(
        State(state.clone()),
        force_logout_principal("tenant-b"),
        Path(sid.clone()),
    )
    .await;
    assert!(matches!(
        cross_tenant,
        Err(ryframe_common::AppError::NotFound(_))
    ));
    assert!(refresh_sessions.is_active(&sid).await.unwrap());
    assert_eq!(
        state
            .services
            .online_user
            .count(&system_actor.actor)
            .await
            .unwrap(),
        1
    );

    let mut connection = redis.conn().clone();
    redis::cmd("CLIENT")
        .arg("PAUSE")
        .arg(2_500)
        .arg("ALL")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    let unavailable = online_user_handler::force_logout(
        State(state.clone()),
        system_actor.clone(),
        Path(sid.clone()),
    )
    .await;
    assert!(matches!(
        unavailable,
        Err(ryframe_common::AppError::ServiceUnavailable(_))
    ));

    tokio::time::sleep(std::time::Duration::from_millis(1_750)).await;
    assert_eq!(
        state
            .services
            .online_user
            .count(&system_actor.actor)
            .await
            .unwrap(),
        1,
        "the secondary index must remain when authoritative revocation reports failure"
    );

    let _response = online_user_handler::force_logout(
        State(state.clone()),
        system_actor.clone(),
        Path(sid.clone()),
    )
    .await
    .unwrap();
    assert!(!refresh_sessions.is_active(&sid).await.unwrap());
    assert_eq!(
        state
            .services
            .online_user
            .count(&system_actor.actor)
            .await
            .unwrap(),
        0
    );

    // The revoked family remains until its absolute expiry, making retries
    // successful even after the index has already been deleted.
    let _response = online_user_handler::force_logout(State(state), system_actor, Path(sid))
        .await
        .unwrap();
}

/// A signed, unexpired access token must not bypass distributed session
/// validation when Redis is unavailable. This exercises the complete Axum
/// middleware stack for an authenticated business route.
#[tokio::test]
#[ignore = "requires Docker Compose MySQL and Redis services"]
async fn auth_middleware_fails_closed_when_redis_is_unavailable() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let port = std::env::var("RYFRAME_TEST_REDIS_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(16379);
    let redis = ryframe_core::RedisClient::connect(&ryframe_config::RedisConfig {
        port,
        database: 12,
        timeout_secs: 1,
        ..Default::default()
    })
    .await
    .expect("connect Docker Compose Redis service");
    let mut state = build_test_app_with_redis(db.connection().clone(), Some(redis.clone())).await;
    let login = state
        .services
        .auth
        .login("system", "admin", "test123")
        .await
        .unwrap();
    assert!(
        state
            .services
            .auth
            .refresh_sessions()
            .is_active(&login.sid)
            .await
            .unwrap()
    );
    let access_claims = ryframe_auth::jwt::decode_token(
        &login.access_token,
        "test-jwt-secret-for-integration-tests",
    )
    .unwrap();
    assert!(access_claims.exp > chrono::Utc::now().timestamp() as usize);

    // Keep the access-JTI blacklist local in this fault injection so the 503
    // specifically proves that the mandatory sid-family lookup fails closed.
    state.auth.blacklist = ryframe_core::TokenBlacklist::new(None);

    let router = api_router(state.clone(), test_rate_limit_state());
    let mut connection = redis.conn().clone();
    redis::cmd("CLIENT")
        .arg("PAUSE")
        .arg(2_500)
        .arg("ALL")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    let request = Request::builder()
        .uri("/system/configs/all")
        .method("GET")
        .header("authorization", format!("Bearer {}", login.access_token))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send_request(router, request).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // Do not leak the pause into the next Redis acceptance test. The failed
    // check must also leave the authoritative family active for a safe retry.
    tokio::time::sleep(std::time::Duration::from_millis(1_750)).await;
    assert!(
        state
            .services
            .auth
            .refresh_sessions()
            .is_active(&login.sid)
            .await
            .unwrap()
    );
}

/// 登出流程
#[tokio::test]
async fn test_logout_flow() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 登出
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let csrf =
        ryframe_auth::jwt::encode_csrf("test-jwt-secret-for-integration-tests", None, 300).unwrap();
    let req = Request::builder()
        .uri("/auth/logout")
        .method("POST")
        .header("authorization", format!("Bearer {}", token))
        .header("x-csrf-token", &csrf)
        .header("cookie", format!("ryframe_csrf={csrf}"))
        .body(Body::empty())
        .unwrap();
    let (status, headers, _) = send_request_with_headers(router.clone(), req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter(|value| value.contains("Max-Age=0"))
            .count(),
        2
    );

    // A repeated logout obtains a fresh unbound challenge after the browser
    // has removed both authentication cookies.
    let repeated_csrf =
        ryframe_auth::jwt::encode_csrf("test-jwt-secret-for-integration-tests", None, 300).unwrap();
    let repeated = Request::builder()
        .uri("/auth/logout")
        .method("POST")
        .header("x-csrf-token", &repeated_csrf)
        .header("cookie", format!("ryframe_csrf={repeated_csrf}"))
        .body(Body::empty())
        .unwrap();
    let (status, headers, _) = send_request_with_headers(router, repeated).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter(|value| value.contains("Max-Age=0"))
            .count(),
        2
    );
}

#[tokio::test]
async fn test_logout_always_requires_csrf_without_refresh_cookie() {
    let db = setup_test_db().await;
    let state = build_test_app(db.connection().clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let request = Request::builder()
        .uri("/auth/logout")
        .method("POST")
        .body(Body::empty())
        .unwrap();

    let (status, _) = send_request(router, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_cookie_logout_revokes_family_without_bearer() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let state = build_test_app(db.connection().clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let login_request = Request::builder()
        .uri("/auth/login")
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "username": "admin",
                "password": "test123"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (status, headers, body) = send_request_with_headers(router.clone(), login_request).await;
    assert_eq!(status, StatusCode::OK);
    let access = body["data"]["access_token"].as_str().unwrap();
    let refresh = response_cookie(&headers, "ryframe_refresh_token").unwrap();
    let claims =
        ryframe_auth::jwt::decode_token(&refresh, "test-jwt-secret-for-integration-tests").unwrap();
    let csrf = ryframe_auth::jwt::encode_csrf(
        "test-jwt-secret-for-integration-tests",
        Some(&claims.sid),
        300,
    )
    .unwrap();
    let logout_request = Request::builder()
        .uri("/auth/logout")
        .method("POST")
        .header("x-csrf-token", &csrf)
        .header(
            "cookie",
            format!("ryframe_refresh_token={refresh}; ryframe_csrf={csrf}"),
        )
        .body(Body::empty())
        .unwrap();
    let (status, logout_headers, _) =
        send_request_with_headers(router.clone(), logout_request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        logout_headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter(|value| value.contains("Max-Age=0"))
            .count(),
        2
    );

    let me_request = Request::builder()
        .uri("/auth/me")
        .method("GET")
        .header("authorization", format!("Bearer {access}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send_request(router, me_request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ==================== 代码生成器测试 ====================

/// 代码生成器：表列表、预览与外部目录写盘
#[tokio::test]
async fn test_generator_endpoints() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 列出数据库表
    let (s, b) = auth_get(&db, "/tools/gen/tables?table_name=sys_user", &token).await;
    assert_eq!(s, StatusCode::OK);
    let tables = b["rows"].as_array().unwrap();
    assert!(!tables.is_empty(), "应至少包含一张表");

    // 预览代码生成（使用确定存在且包含单主键的业务表）
    let table_name = tables
        .iter()
        .find_map(|table| {
            (table["table_name"] == "sys_user").then(|| table["table_name"].as_str().unwrap())
        })
        .unwrap_or_else(|| panic!("表列表应包含 sys_user，实际为: {tables:?}"));
    let (s, body) = auth_post(
        &db,
        "/tools/gen/preview",
        &token,
        serde_json::json!({
            "tables": [table_name]
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["data"].as_array().map(Vec::len), Some(5));

    let output = tempfile::tempdir().unwrap();
    let output_root = output.path().join("generated");
    let (s, body) = auth_post(
        &db,
        "/tools/gen/generate",
        &token,
        serde_json::json!({
            "output_dir": output_root,
            "options": {
                "tables": [table_name]
            }
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "生成响应: {body:?}");
    let written = body["data"]["written"].as_array().unwrap();
    assert_eq!(written.len(), 5);
    assert!(written.iter().all(|path| {
        path.as_str()
            .is_some_and(|path| output_root.join(path).is_file())
    }));
}

// ================================================================
// P2 新增功能集成测试
// ================================================================

/// API 版本信息端点
#[tokio::test]
async fn test_version_endpoint() {
    let db = setup_test_db().await;
    let state = build_test_app(db.connection().clone()).await;
    let router = api_router(state, test_rate_limit_state());

    let req = Request::builder()
        .uri("/version")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let (status, body) = send_request(router, req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "ryframe-api");
    assert!(body["version"].is_string());
    assert_eq!(body["api_prefix"], "/api/v1");
    assert!(body["endpoints"].is_object());
}

/// Swagger UI 文档页面
#[tokio::test]
async fn test_swagger_ui_endpoint() {
    let db = setup_test_db().await;
    let state = build_test_app(db.connection().clone()).await;
    let router = api_router(state, test_rate_limit_state());

    let req = Request::builder()
        .uri("/swagger-ui")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(content_type.contains("text/html"), "Swagger UI 应返回 HTML");
}

/// OpenAPI JSON 文档端点
#[tokio::test]
async fn test_openapi_json_endpoint() {
    let db = setup_test_db().await;
    let state = build_test_app(db.connection().clone()).await;
    let router = api_router(state, test_rate_limit_state());

    let req = Request::builder()
        .uri("/api-docs/openapi.json")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let (status, body) = send_request(router, req).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["info"].is_object(), "OpenAPI JSON 应包含 info");
    assert!(body["paths"].is_object(), "OpenAPI JSON 应包含 paths");

    let paths = body["paths"]
        .as_object()
        .expect("paths should be an object");
    assert!(
        paths.contains_key("/api/v1/system/users/{id}/password-reset-requests"),
        "OpenAPI 应包含新的密码重置请求接口"
    );
    assert!(
        paths.contains_key("/api/v1/system/users/{id}/roles"),
        "OpenAPI 应包含用户角色子资源接口"
    );
    assert!(
        paths.contains_key("/api/v1/system/users/{id}/status"),
        "OpenAPI 应包含用户状态子资源接口"
    );
    assert!(
        paths.contains_key("/api/v1/auth/password-reset/complete"),
        "OpenAPI 应包含公开密码重置完成接口"
    );
    assert!(
        !paths.contains_key("/api/v1/system/users/{id}/password"),
        "OpenAPI 不应再暴露旧管理员重置密码接口"
    );
    assert!(
        !paths.contains_key("/api/v1/system/users/assign-role"),
        "OpenAPI 不应再暴露旧用户角色动作接口"
    );
    assert!(
        !paths.contains_key("/api/v1/system/users/status"),
        "OpenAPI 不应再暴露旧用户状态动作接口"
    );
    assert!(
        !paths.contains_key("/api/v1/system/operlogs/clean"),
        "OpenAPI 不应暴露操作日志清空接口"
    );
    assert!(
        !paths.contains_key("/api/v1/system/loginlogs/clean"),
        "OpenAPI 不应暴露登录日志清空接口"
    );
    assert!(
        !paths.contains_key("/api/v1/system/roles/{id}/menus"),
        "OpenAPI 不应再暴露旧角色菜单分配接口"
    );
}

/// Token 黑名单：登出后 Token 失效
#[tokio::test]
async fn test_token_blacklist_on_logout() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;

    // 1. 登录获取 token
    let token = login_get_token(&db).await;
    assert!(!token.is_empty());

    // 2. 使用 token 访问 /auth/me (应成功)
    let (s, b) = auth_get(&db, "/auth/me", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["username"], "admin");

    // 3. 构建一次 state，用于登出和后续验证（复用同一个 TokenBlacklist）
    let state = build_test_app(db.clone()).await;
    let router = api_router(state.clone(), test_rate_limit_state());

    let csrf =
        ryframe_auth::jwt::encode_csrf("test-jwt-secret-for-integration-tests", None, 300).unwrap();
    let logout_req = Request::builder()
        .uri("/auth/logout")
        .method("POST")
        .header("authorization", format!("Bearer {}", token))
        .header("x-csrf-token", &csrf)
        .header("cookie", format!("ryframe_csrf={csrf}"))
        .body(Body::empty())
        .unwrap();
    let (s, _) = send_request(router, logout_req).await;
    assert_eq!(s, StatusCode::OK);

    // 4. 登出后再次使用同一 token（同一 state）访问 /auth/me (应 401)
    let router2 = api_router(state, test_rate_limit_state());
    let me_req = Request::builder()
        .uri("/auth/me")
        .method("GET")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let (s, _) = send_request(router2, me_req).await;
    assert_eq!(
        s,
        StatusCode::UNAUTHORIZED,
        "登出后 token 应被加入黑名单并返回 401"
    );
}

/// 认证后访问系统管理接口（验证完整中间件链路）
#[tokio::test]
async fn test_authenticated_system_access() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 使用有效 token 访问各系统端点
    let endpoints = vec![
        "/system/users?page=1&page_size=5",
        "/system/roles?page=1&page_size=5",
        "/system/depts/tree",
        "/system/menus/tree",
        "/system/perms/tree",
        "/system/online",
        "/system/posts/all",
    ];
    for uri in endpoints {
        let (s, _) = auth_get(&db, uri, &token).await;
        assert_eq!(s, StatusCode::OK, "端点 {} 应返回 200", uri);
    }
}

/// 无效 Token 访问受保护资源
#[tokio::test]
async fn test_invalid_token_rejected() {
    let db = setup_test_db().await;
    let fake_token = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIiwianRpIjoiZmFrZSJ9.fake";

    let state = build_test_app(db.connection().clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri("/auth/me")
        .method("GET")
        .header("authorization", format!("Bearer {}", fake_token))
        .body(Body::empty())
        .unwrap();
    let (s, _) = send_request(router, req).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}
