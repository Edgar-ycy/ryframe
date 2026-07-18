//! 集成测试共享 Fixture 模块
//!
//! 提供数据库初始化、测试数据填充、认证辅助函数。
//! 在集成测试文件中通过 `mod common;` 引用。

use std::sync::Arc;

#[path = "../../../ryframe-db/tests/common/test_database.rs"]
mod test_database;

pub use test_database::TestDatabase;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use ryframe_api::{AppServices, AppState, router::api_router, runtime::RuntimeComponents};
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
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Schema};
use std::net::SocketAddr;
use tower::ServiceExt;

// ==================== 数据库 ====================

/// 创建隔离的 MySQL 8.4 测试数据库并建表。
pub async fn setup_test_db() -> TestDatabase {
    let db = TestDatabase::create("api").await;
    create_all_tables(&db).await;
    db
}

/// 为所有测试用到的实体创建表
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
    create!(ryframe_db::entities::config::Entity);
    create!(ryframe_db::entities::user_role::Entity);
    create!(ryframe_db::entities::role_permission::Entity);
    create!(ryframe_db::entities::role_dept::Entity);
    create!(ryframe_db::entities::sys_file::Entity);
}

/// 填充测试数据：管理员 + 部门 + 角色
pub async fn seed_test_data(db: &DatabaseConnection) {
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
    seed_user(db, 1, "admin", "管理员", Some(1)).await;

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

    // 创建普通用户角色
    let normal_role = role::Model {
        id: 2,
        tenant_id: "system".into(),
        name: "普通用户".into(),
        code: "user".into(),
        is_super: 0,
        data_scope: "5".into(),
        status: "1".into(),
        sort: 1,
        remark: None,
        del_flag: role::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let active: role::ActiveModel = normal_role.into();
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

    // 分配角色给 admin
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

/// 快速创建一个测试用户
pub async fn seed_user(
    db: &DatabaseConnection,
    id: i64,
    username: &str,
    nickname: &str,
    dept_id: Option<i64>,
) {
    let password_hash = ryframe_auth::password::hash("test123").unwrap();
    let user_model = user::Model {
        id,
        tenant_id: "system".into(),
        username: username.to_string(),
        password_hash,
        nickname: nickname.to_string(),
        email: format!("{}@test.com", username),
        phone: "13800000000".to_string(),
        avatar: None,
        status: user::Model::STATUS_NORMAL.to_string(),
        auth_version: 1,
        dept_id,
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let active: user::ActiveModel = user_model.into();
    user::Entity::insert(active).exec(db).await.unwrap();
}

// ==================== 配置 ====================

/// 创建测试用的 AppConfig
pub fn test_config() -> AppConfig {
    AppConfig {
        app: AppSettings {
            name: "test".into(),
            port: 0,
            ..Default::default()
        },
        database: DatabaseConfig {
            primary: DbConnection {
                database: "ryframe_api_test".into(),
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

/// 创建测试用的 RateLimitState（限流默认关闭，不会拦截测试请求）
pub fn test_rate_limit_state() -> RateLimitState {
    RateLimitState {
        limiter: Arc::new(ryframe_middleware::RateLimiter::new_in_memory(100, 10)),
        config: Arc::new(RateLimitConfig::default()),
        trusted_proxies: Default::default(),
    }
}

// ==================== App 构建 ====================

/// 构建测试用 AppState 和 Router
pub async fn build_test_app(db: DatabaseConnection) -> AppState {
    let config = test_config();
    let config_arc = Arc::new(config.clone());
    let token_blacklist = ryframe_core::TokenBlacklist::new(None);
    let database = DatabaseCluster::single(db.clone());
    let auth_service = Arc::new(AuthService::new(database.clone(), config_arc.clone(), None));
    AppState {
        auth: ryframe_auth::middleware::AuthState {
            config: config_arc.clone(),
            blacklist: token_blacklist.clone(),
            refresh_sessions: auth_service.refresh_sessions(),
            principal_resolver: auth_service.clone(),
        },
        monitor: ryframe_monitor::MonitorState {
            database: Arc::new(ryframe_db::SeaOrmDatabaseMonitor::new(database.clone())),
            redis: None,
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
            online_user: Arc::new(OnlineUserService::new_in_memory()),
            captcha: CaptchaStore::new_in_memory(300),
        }),
        redis: None,
        token_blacklist,
        rate_limiter: Arc::new(ryframe_middleware::RateLimiter::new_in_memory(100, 10)),
        trusted_proxies: Default::default(),
        runtime: RuntimeComponents::new(None),
    }
}

// ==================== HTTP 辅助 ====================

/// 发送请求并返回 (StatusCode, Body JSON)
pub async fn send_request(
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

pub async fn send_request_with_headers(
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

pub fn response_cookie(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .filter_map(|value| value.split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.to_owned()))
}

/// 登录并返回 access_token
pub async fn login_get_token(db: &DatabaseConnection) -> String {
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

/// 带认证的 GET 请求
pub async fn auth_get(
    db: &DatabaseConnection,
    uri: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri(uri)
        .method("GET")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    send_request(router, req).await
}

/// 带认证的 POST 请求
pub async fn auth_post(
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
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    send_request(router, req).await
}

/// 带认证的 PUT 请求
pub async fn auth_put(
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
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    send_request(router, req).await
}

/// 带认证的 DELETE 请求
pub async fn auth_delete(
    db: &DatabaseConnection,
    uri: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri(uri)
        .method("DELETE")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    send_request(router, req).await
}
