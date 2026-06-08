//! 集成测试共享 Fixture 模块
//!
//! 提供数据库初始化、测试数据填充、认证辅助函数。
//! 在集成测试文件中通过 `mod common;` 引用。

use std::sync::Arc;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use ryframe_api::{
    handlers::{auth_handler::AppState, captcha_handler::CaptchaStore},
    router::api_router,
};
use ryframe_config::{
    AppConfig, AppSettings, AuthConfig, DatabaseConfig, DbConnection, LoggerConfig, RateLimitConfig,
};
use ryframe_core::{AppContext, LoggedRepo};
use ryframe_db::{
    ConfigRepository, DeptRepository, DictDataRepository, DictTypeRepository, JobLogRepository,
    JobRepository, LoginInfoRepository, MenuRepository, NoticeRepository, OperLogRepository,
    PermissionRepository, PostRepository, RoleRepository, UserRepository,
    entities::{dept, role, user},
};
use ryframe_middleware::rate_limit::RateLimitState;
use ryframe_service::{
    AuthServiceImpl,
    system::{
        ConfigServiceImpl, DeptServiceImpl, DictServiceImpl, GeneratorServiceImpl, JobServiceImpl,
        LoginInfoServiceImpl, MenuServiceImpl, NoticeServiceImpl, OnlineUserServiceImpl,
        OperLogServiceImpl, PermissionServiceImpl, PostServiceImpl, ProfileServiceImpl,
        RoleServiceImpl, UserServiceImpl,
    },
};
use ryframe_task::{TaskContext, TaskScheduler};
use sea_orm::{Database, DatabaseConnection, EntityTrait};
use sea_orm_migration::MigratorTrait;
use std::net::SocketAddr;
use tower::ServiceExt;

// ==================== 数据库 ====================

/// 创建 SQLite 内存数据库并运行迁移
pub async fn setup_test_db() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("连接 SQLite 内存数据库失败");

    ryframe_db::migration::Migrator::up(&db, None)
        .await
        .expect("数据库迁移失败");

    db
}

/// 填充测试数据：管理员 + 部门 + 角色
pub async fn seed_test_data(db: &DatabaseConnection) {
    // 创建根部门
    let dept_model = dept::Model {
        id: 1,
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
        name: "超级管理员".into(),
        code: "admin".into(),
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
        name: "普通用户".into(),
        code: "user".into(),
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

    // 分配角色给 admin
    let user_role_model = ryframe_db::entities::user_role::Model {
        user_id: 1,
        role_id: 1,
    };
    let active: ryframe_db::entities::user_role::ActiveModel = user_role_model.into();
    ryframe_db::entities::user_role::Entity::insert(active)
        .exec(db)
        .await
        .unwrap();
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
        username: username.to_string(),
        password_hash,
        nickname: nickname.to_string(),
        email: format!("{}@test.com", username),
        phone: "13800000000".to_string(),
        avatar: None,
        status: user::Model::STATUS_NORMAL.to_string(),
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
            connections: vec![DbConnection {
                driver: "sqlite".into(),
                database: ":memory:".into(),
                max_connections: 5,
                ..Default::default()
            }],
            ..Default::default()
        },
        auth: AuthConfig {
            jwt_secret: "test-jwt-secret-for-integration-tests".into(),
            enable_password_complexity: false,
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
    }
}

/// 创建测试用的 RateLimitState（限流默认关闭，不会拦截测试请求）
pub fn test_rate_limit_state() -> RateLimitState {
    RateLimitState {
        limiter: Arc::new(ryframe_middleware::RateLimiter::new_in_memory(100, 10)),
        config: Arc::new(RateLimitConfig::default()),
    }
}

// ==================== App 构建 ====================

/// 构建测试用 AppState 和 Router
pub async fn build_test_app(db: DatabaseConnection) -> AppState {
    let config = test_config();
    let config_arc = Arc::new(config.clone());
    let context = AppContext::new(config.clone());
    let task_ctx = TaskContext {
        db: Arc::new(db.clone()),
    };
    let scheduler = Arc::new(TaskScheduler::new(task_ctx));

    AppState {
        datasource_manager: Default::default(),
        db: db.clone(),
        config: config_arc,
        context,
        auth_service: Arc::new(AuthServiceImpl {
            user_repo: LoggedRepo::new(UserRepository),
            role_repo: LoggedRepo::new(RoleRepository),
            perm_repo: LoggedRepo::new(PermissionRepository),
            config: Arc::new(config.clone()),
        }),
        user_service: Arc::new(UserServiceImpl {
            user_repo: LoggedRepo::new(UserRepository),
            role_repo: LoggedRepo::new(RoleRepository),
            dept_repo: LoggedRepo::new(DeptRepository),
        }),
        role_service: Arc::new(RoleServiceImpl {
            role_repo: LoggedRepo::new(RoleRepository),
            perm_repo: LoggedRepo::new(PermissionRepository),
            menu_repo: LoggedRepo::new(MenuRepository),
        }),
        permission_service: Arc::new(PermissionServiceImpl {
            perm_repo: LoggedRepo::new(PermissionRepository),
        }),
        menu_service: Arc::new(MenuServiceImpl {
            menu_repo: LoggedRepo::new(MenuRepository),
            redis: None,
        }),
        dept_service: Arc::new(DeptServiceImpl {
            dept_repo: LoggedRepo::new(DeptRepository),
            redis: None,
        }),
        post_service: Arc::new(PostServiceImpl {
            post_repo: LoggedRepo::new(PostRepository),
        }),
        config_service: Arc::new(ConfigServiceImpl {
            config_repo: LoggedRepo::new(ConfigRepository),
            redis: None,
        }),
        dict_service: Arc::new(DictServiceImpl {
            dict_type_repo: LoggedRepo::new(DictTypeRepository),
            dict_data_repo: LoggedRepo::new(DictDataRepository),
            redis: None,
        }),
        notice_service: Arc::new(NoticeServiceImpl {
            notice_repo: LoggedRepo::new(NoticeRepository),
        }),
        oper_log_service: Arc::new(OperLogServiceImpl {
            oper_log_repo: LoggedRepo::new(OperLogRepository),
        }),
        login_info_service: Arc::new(LoginInfoServiceImpl {
            login_info_repo: LoggedRepo::new(LoginInfoRepository),
        }),
        job_service: Arc::new(JobServiceImpl {
            job_repo: LoggedRepo::new(JobRepository),
            job_log_repo: LoggedRepo::new(JobLogRepository),
            scheduler: scheduler.clone(),
        }),
        generator_service: Arc::new(GeneratorServiceImpl {
            workspace_root: std::env::current_dir().unwrap(),
        }),
        profile_service: Arc::new(ProfileServiceImpl {
            user_repo: LoggedRepo::new(UserRepository),
            role_repo: LoggedRepo::new(RoleRepository),
            perm_repo: LoggedRepo::new(PermissionRepository),
        }),
        online_user_service: Arc::new(OnlineUserServiceImpl::new_in_memory()),
        captcha_store: CaptchaStore::new_in_memory(300),
        scheduler,
        monitor_db: db,
        redis: None,
        token_blacklist: ryframe_core::TokenBlacklist::new(None),
        replica_dbs: vec![],
        rate_limiter: Arc::new(ryframe_middleware::RateLimiter::new_in_memory(100, 10)),
        object_storage: Arc::new(ryframe_common::utils::LocalObjectStorage::new(
            "uploads", "",
        )),
    }
}

// ==================== HTTP 辅助 ====================

/// 发送请求并返回 (StatusCode, Body JSON)
pub async fn send_request(
    app: axum::Router,
    mut req: Request<Body>,
) -> (StatusCode, serde_json::Value) {
    // Axum 0.8: oneshot() 不自动注入 ConnectInfo，需手动 mock
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:8080".parse::<SocketAddr>().unwrap()));
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    (status, json)
}

/// 登录并返回 access_token
pub async fn login_get_token(db: &DatabaseConnection) -> String {
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri("/auth/login")
        .method("POST")
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
