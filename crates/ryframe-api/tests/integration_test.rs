//! API 集成测试
//!
//! 使用 SQLite 内存数据库 + axum test client 测试端到端流程。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sea_orm::{Database, DatabaseConnection, EntityTrait};
use sea_orm_migration::MigratorTrait;
use std::sync::Arc;
use tower::ServiceExt;

use ryframe_api::handlers::auth_handler::AppState;
use ryframe_api::handlers::captcha_handler::CaptchaStore;
use ryframe_api::router::api_router;
use ryframe_config::{
    AppConfig, AppSettings, AuthConfig, DatabaseConfig, DbConnection, LoggerConfig, RateLimitConfig,
};
use ryframe_core::{AppContext, LoggedRepo};
use ryframe_db::entities::{dept, role, user};
use ryframe_db::{
    ConfigRepository, DeptRepository, DictDataRepository, DictTypeRepository, JobLogRepository,
    JobRepository, LoginInfoRepository, MenuRepository, NoticeRepository, OperLogRepository,
    PermissionRepository, PostRepository, RoleRepository, UserRepository,
};
use ryframe_service::AuthServiceImpl;
use ryframe_service::system::{
    ConfigServiceImpl, DeptServiceImpl, DictServiceImpl, GeneratorServiceImpl, JobServiceImpl,
    LoginInfoServiceImpl, MenuServiceImpl, NoticeServiceImpl, OnlineUserServiceImpl,
    OperLogServiceImpl, PermissionServiceImpl, PostServiceImpl, ProfileServiceImpl,
    RoleServiceImpl, UserServiceImpl,
};
use ryframe_task::{TaskContext, TaskScheduler};

/// 创建 SQLite 内存数据库并运行迁移
async fn setup_test_db() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("连接 SQLite 内存数据库失败");

    ryframe_db::migration::Migrator::up(&db, None)
        .await
        .expect("数据库迁移失败");

    db
}

/// 填充测试数据：管理员 + 部门
async fn seed_test_data(db: &DatabaseConnection) {
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
    let password_hash = ryframe_auth::password::hash("test123").unwrap();
    let user_model = user::Model {
        id: 1,
        username: "admin".into(),
        password_hash,
        nickname: "管理员".into(),
        email: "admin@test.com".into(),
        phone: "13800000000".into(),
        avatar: None,
        status: user::Model::STATUS_NORMAL.to_string(),
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

    // 分配角色
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

fn test_config() -> AppConfig {
    AppConfig {
        app: AppSettings {
            name: "test".into(),
            version: "0.1.0".into(),
            host: "127.0.0.1".into(),
            port: 0,
        },
        database: DatabaseConfig {
            primary: DbConnection {
                driver: "sqlite".into(),
                host: "".into(),
                port: 0,
                database: ":memory:".into(),
                username: "".into(),
                password: "".into(),
                max_connections: 5,
                min_connections: 1,
            },
            replicas: vec![],
            datasources: vec![],
            sql_log_level: ryframe_config::SqlLogLevel::Off,
        },
        auth: AuthConfig {
            jwt_secret: "test-jwt-secret-for-integration-tests".into(),
            access_token_expire: "1h".into(),
            refresh_token_expire: "168h".into(),
        },
        redis: None,
        logger: LoggerConfig {
            level: "warn".into(),
            format: "text".into(),
            output: "stdout".into(),
        },
        rate_limit: RateLimitConfig::default(),
        cors: Default::default(),
    }
}

async fn build_test_app(db: DatabaseConnection) -> AppState {
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
        }),
        dept_service: Arc::new(DeptServiceImpl {
            dept_repo: LoggedRepo::new(DeptRepository),
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
        replica_dbs: vec![],
    }
}

/// 辅助：发送请求并返回 (StatusCode, Body JSON)
async fn send_request(app: axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    (status, json)
}

// ==================== 测试用例 ====================

#[tokio::test]
async fn test_health_check() {
    let db = setup_test_db().await;
    let state = build_test_app(db).await;
    let router = api_router(state);

    let req = Request::builder()
        .uri("/auth/login")
        .method("OPTIONS")
        .body(Body::empty())
        .unwrap();
    let _ = router.oneshot(req).await;
}

/// 认证全流程：登录 → me → 刷新 → 错误场景
#[tokio::test]
async fn test_auth_full_flow() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;

    // 1. 登录成功
    let state = build_test_app(db.clone()).await;
    let router = api_router(state);
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
    let access_token = body["data"]["access_token"].as_str().unwrap().to_string();
    let refresh_token = body["data"]["refresh_token"].as_str().unwrap().to_string();
    assert!(!access_token.is_empty());
    assert_eq!(body["data"]["user_info"]["username"], "admin");

    // 2. 用 token 访问 /auth/me
    let state2 = build_test_app(db.clone()).await;
    let router2 = api_router(state2);
    let me_req = Request::builder()
        .uri("/auth/me")
        .method("GET")
        .header("authorization", format!("Bearer {}", access_token))
        .body(Body::empty())
        .unwrap();
    let (s2, b2) = send_request(router2, me_req).await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(b2["data"]["username"], "admin");

    // 3. 刷新令牌
    let state3 = build_test_app(db.clone()).await;
    let router3 = api_router(state3);
    let refresh_req = Request::builder()
        .uri("/auth/refresh")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "refresh_token": refresh_token
            }))
            .unwrap(),
        ))
        .unwrap();
    let (s3, b3) = send_request(router3, refresh_req).await;
    assert_eq!(s3, StatusCode::OK);
    assert!(b3["data"].get("access_token").is_some());

    // 4. 错误密码
    let state4 = build_test_app(db.clone()).await;
    let router4 = api_router(state4);
    let bad_req = Request::builder()
        .uri("/auth/login")
        .method("POST")
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
    let router5 = api_router(state5);
    let notfound_req = Request::builder()
        .uri("/auth/login")
        .method("POST")
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
    let state6 = build_test_app(db).await;
    let router6 = api_router(state6);
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
    let router = api_router(state);
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

/// 辅助：发送带认证的 GET 请求
async fn auth_get(
    db: &DatabaseConnection,
    uri: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let state = build_test_app(db.clone()).await;
    let router = api_router(state);
    let req = Request::builder()
        .uri(uri)
        .method("GET")
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
    let router = api_router(state);
    let req = Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(serde_json::to_string(&body).unwrap()))
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
    let (s, _) = auth_get(&db, "/system/posts/list?page=1&pageSize=10", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_get(&db, "/system/posts/listNoPage", &token).await;
    assert_eq!(s, StatusCode::OK);

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
    let (s, _) = auth_get(&db, "/system/configs/list?page=1&pageSize=10", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_get(&db, "/system/configs/listNoPage", &token).await;
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
    let (s, _) = auth_get(&db, "/system/dict/types/list?page=1&pageSize=10", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_get(&db, "/system/dict/types/listNoPage", &token).await;
    assert_eq!(s, StatusCode::OK);

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
    let (s, _) = auth_get(&db, "/system/notices/list?page=1&pageSize=10", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_get(&db, "/system/notices/listNoPage", &token).await;
    assert_eq!(s, StatusCode::OK);
}

/// 系统查询接口：用户/角色/部门/菜单/权限/在线用户
#[tokio::test]
async fn test_system_query_endpoints() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    let (s, b) = auth_get(&db, "/system/users/list?page=1&pageSize=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some());

    let (s, _) = auth_get(&db, "/system/users/listNoPage", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, b) = auth_get(&db, "/system/roles/list?page=1&pageSize=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some());

    let (s, _) = auth_get(&db, "/system/roles/listNoPage", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, b) = auth_get(&db, "/system/depts/tree", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["data"].as_array().is_some());

    let (s, b) = auth_get(&db, "/system/depts/list?page=1&pageSize=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some());

    let (s, _) = auth_get(&db, "/system/depts/listNoPage", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, b) = auth_get(&db, "/system/menus/tree", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["data"].as_array().is_some());

    let (s, b) = auth_get(&db, "/system/menus/list?page=1&pageSize=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some());

    let (s, _) = auth_get(&db, "/system/menus/listNoPage", &token).await;
    assert_eq!(s, StatusCode::OK);

    let (s, b) = auth_get(&db, "/system/permissions/tree", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["data"].as_array().is_some());

    let (s, b) = auth_get(&db, "/system/online", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["data"].as_array().is_some());
}

/// 未认证访问系统接口应返回 401
#[tokio::test]
async fn test_unauthenticated_access_denied() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;

    let state = build_test_app(db.clone()).await;
    let router = api_router(state);
    let endpoints = vec![
        "/system/users/list?page=1&pageSize=10",
        "/system/roles/list?page=1&pageSize=10",
        "/system/depts/tree",
        "/system/depts/list?page=1&pageSize=10",
        "/system/posts/list?page=1&pageSize=10",
        "/system/configs/list?page=1&pageSize=10",
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
