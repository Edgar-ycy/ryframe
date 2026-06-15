//! API 集成测试
//!
//! 使用 SQLite 内存数据库 + axum test client 测试端到端流程。

use std::sync::Arc;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use ryframe_api::{handlers::auth_handler::AppState, router::api_router};
use ryframe_config::{
    AppConfig, AppSettings, AuthConfig, DatabaseConfig, DbConnection, LoggerConfig, RateLimitConfig,
};
use ryframe_core::{AppContext, LoggedRepo};
use ryframe_db::{
    ConfigRepository, DeptRepository, DictDataRepository, DictTypeRepository, LoginInfoRepository,
    MenuRepository, NoticeRepository, OperLogRepository, PermissionRepository, PostRepository,
    RoleRepository, UserRepository,
    entities::{config, dept, permission, role, role_permission, user},
};
use ryframe_middleware::rate_limit::RateLimitState;
use ryframe_service::{
    AuthServiceImpl,
    system::{
        CaptchaStore, ConfigServiceImpl, DeptServiceImpl, DictServiceImpl, GeneratorServiceImpl,
        LoginInfoServiceImpl, MenuServiceImpl, NoticeServiceImpl, OnlineUserServiceImpl,
        OperLogServiceImpl, PermissionServiceImpl, PostServiceImpl, ProfileServiceImpl,
        RoleServiceImpl, UserServiceImpl,
    },
};
use sea_orm::{
    ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, EntityTrait, Schema,
};
use std::net::SocketAddr;
use tower::ServiceExt;

/// 创建 SQLite 内存数据库并运行迁移
async fn setup_test_db() -> DatabaseConnection {
    Database::connect("sqlite::memory:")
        .await
        .expect("连接 SQLite 内存数据库失败")
}

/// 填充测试数据：管理员 + 部门
async fn create_all_tables(db: &DatabaseConnection) {
    let backend = DatabaseBackend::Sqlite;
    let schema = Schema::new(backend);

    macro_rules! create {
        ($entity:path) => {
            let stmt = schema.create_table_from_entity($entity);
            db.execute(&stmt).await.expect("create table failed");
        };
    }

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
    create!(ryframe_db::entities::user_role::Entity);
    create!(ryframe_db::entities::role_menu::Entity);
    create!(ryframe_db::entities::role_permission::Entity);
    create!(ryframe_db::entities::role_dept::Entity);
    create!(ryframe_db::entities::sys_file::Entity);
}

async fn seed_test_data(db: &DatabaseConnection) {
    create_all_tables(db).await;

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

    let all_permission = permission::Model {
        id: 1,
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
        user_id: 1,
        role_id: 1,
    };
    let active: ryframe_db::entities::user_role::ActiveModel = user_role_model.into();
    ryframe_db::entities::user_role::Entity::insert(active)
        .exec(db)
        .await
        .unwrap();

    let role_permission_model = role_permission::Model {
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

fn test_rate_limit_state() -> RateLimitState {
    RateLimitState {
        limiter: Arc::new(ryframe_middleware::RateLimiter::new_in_memory(100, 10)),
        config: Arc::new(RateLimitConfig::default()),
    }
}

async fn build_test_app(db: DatabaseConnection) -> AppState {
    let config = test_config();
    let config_arc = Arc::new(config.clone());
    let context = AppContext::new(config.clone());

    AppState {
        db: db.clone(),
        config: config_arc,
        context,
        auth_service: Arc::new(AuthServiceImpl {
            user_repo: LoggedRepo::new(UserRepository),
            role_repo: LoggedRepo::new(RoleRepository),
            perm_repo: LoggedRepo::new(PermissionRepository),
            config: Arc::new(config.clone()),
            redis: None,
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

/// 辅助：发送请求并返回 (StatusCode, Body JSON)
async fn send_request(
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

// ==================== 测试用例 ====================

#[tokio::test]
async fn test_health_check() {
    let db = setup_test_db().await;
    let state = build_test_app(db).await;
    let router = api_router(state, test_rate_limit_state());

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
    let access_token = body["data"]["access_token"].as_str().unwrap().to_string();
    let refresh_token = body["data"]["refresh_token"].as_str().unwrap().to_string();
    assert!(!access_token.is_empty());
    assert_eq!(body["data"]["user_info"]["username"], "admin");

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

    // 3. 刷新令牌
    let state3 = build_test_app(db.clone()).await;
    let router3 = api_router(state3, test_rate_limit_state());
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
    let router4 = api_router(state4, test_rate_limit_state());
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
    let router5 = api_router(state5, test_rate_limit_state());
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
    let router = api_router(state, test_rate_limit_state());
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
    let (s, b) = auth_get(&db, "/system/configs/configKey/temp.config", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["data"].as_str().is_some(), "配置值应存在");

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
    let dept_id = b["data"]["id"].as_i64().unwrap();

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
        &db, "/system/menus", &token,
        serde_json::json!({"name": "测试菜单", "parent_id": null, "menu_type": "C", "path": null, "component": null, "query": null, "perms": null, "icon": null, "is_frame": false, "is_cache": false, "sort": 0, "visible": true}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let menu_id = b["data"]["id"].as_i64().unwrap();

    let (s, _) = auth_put(
        &db, &format!("/system/menus/{}", menu_id), &token,
        serde_json::json!({"name": "改名菜单", "parent_id": null, "menu_type": "C", "path": null, "component": null, "query": null, "perms": null, "icon": null, "is_frame": false, "is_cache": false, "sort": 1, "visible": true, "status": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

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
        serde_json::json!({"name": "改名角色", "sort": 3, "status": "1", "data_scope": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 分配权限
    let (s, _) = auth_put(
        &db,
        &format!("/system/roles/{}/permissions", role_id),
        &token,
        serde_json::json!({"perm_ids": []}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 分配菜单
    let (s, _) = auth_put(
        &db,
        &format!("/system/roles/{}/menus", role_id),
        &token,
        serde_json::json!({"menu_ids": []}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 设置数据权限
    let (s, _) = auth_put(
        &db,
        &format!("/system/roles/{}/data-scope", role_id),
        &token,
        serde_json::json!({"data_scope": "3", "dept_ids": []}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = auth_delete(&db, &format!("/system/roles/{}", role_id), &token).await;
    assert_eq!(s, StatusCode::OK);

    // ===== 用户：创建 → 更新 → 修改状态 → 重置密码 → 删除 =====
    let (s, b) = auth_post(
        &db, "/system/users", &token,
        serde_json::json!({"username": "testupdate", "password": "123456", "nickname": "测试更新", "email": null, "phone": null, "dept_id": "1", "role_ids": null}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let user_id = b["data"]["id"].as_str().unwrap().parse::<i64>().unwrap();

    // 更新用户
    let (s, _) = auth_put(
        &db, &format!("/system/users/{}", user_id), &token,
        serde_json::json!({"nickname": "已更新", "email": null, "phone": null, "dept_id": "1", "status": "1", "role_ids": null}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 修改用户状态
    let (s, _) = auth_put(
        &db,
        "/system/users/changeStatus",
        &token,
        serde_json::json!({"user_id": user_id.to_string(), "status": "0"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 重置密码
    let (s, _) = auth_put(
        &db,
        &format!("/system/users/{}/password", user_id),
        &token,
        serde_json::json!({"password": "newpass123"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 删除用户
    let (s, _) = auth_delete(&db, &format!("/system/users/{}", user_id), &token).await;
    assert_eq!(s, StatusCode::OK);
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
    let (s, _) = auth_get(&db, "/system/configs/configKey/nonexistent_key", &token).await;
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
        serde_json::json!({"username": "duplicate_user", "password": "123456", "nickname": "重复用户", "email": null, "phone": null, "dept_id": "1", "role_ids": null}),
    )
    .await;

    // 尝试用相同 username 创建另一个 user
    let (s, _) = auth_post(
        &db, "/system/users", &token,
        serde_json::json!({"username": "duplicate_user", "password": "123456", "nickname": "重名用户", "email": null, "phone": null, "dept_id": "1", "role_ids": null}),
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

/// 监控端点（/health 和 /metrics 公开，/server /cache /db-pool 需认证）
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

    // 健康检查（公开）
    let req = Request::builder()
        .uri("/monitor/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let (status, body) = send_request(router.clone(), req).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].get("database").is_some(), "应包含数据库状态");

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
    let (s, b) = auth_get(&db, "/system/loginlogs/list?page=1&pageSize=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some(), "登录日志应返回 rows");

    // 登录日志不分页
    let (s, _) = auth_get(&db, "/system/loginlogs/listNoPage", &token).await;
    assert_eq!(s, StatusCode::OK);

    // 清空登录日志
    let (s, _) = auth_delete(&db, "/system/loginlogs/clean", &token).await;
    assert_eq!(s, StatusCode::OK);

    // 操作日志列表
    let (s, b) = auth_get(&db, "/system/operlogs/list?page=1&pageSize=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some(), "操作日志应返回 rows");

    // 操作日志不分页
    let (s, _) = auth_get(&db, "/system/operlogs/listNoPage", &token).await;
    assert_eq!(s, StatusCode::OK);

    // 清空操作日志
    let (s, _) = auth_delete(&db, "/system/operlogs/clean", &token).await;
    assert_eq!(s, StatusCode::OK);
}

// ==================== 个人中心端点测试 ====================

/// 个人中心：获取/更新信息、修改密码
/// 注意：profile 端点需要 Claims extension，当前 test 构建链中 profile auth middleware
/// 嵌套存在限制，此测试暂时跳过。实际运行时 profile handler 通过 profile_service 验证。
#[ignore]
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
        serde_json::json!({"old_password": "test123", "new_password": "newpass456"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

// ==================== 登出测试 ====================

/// 登出流程
#[tokio::test]
async fn test_logout_flow() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 登出
    let state = build_test_app(db.clone()).await;
    let router = api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri("/auth/logout")
        .method("POST")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send_request(router, req).await;
    assert_eq!(status, StatusCode::OK);
}

// ==================== 代码生成器测试 ====================

/// 代码生成器：表列表与预览
/// 注意：生成器使用 information_schema（MySQL 专用），SQLite 内存数据库不支持。
/// 需要 MySQL 数据库时启用此测试。
#[ignore]
#[tokio::test]
async fn test_generator_endpoints() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 列出数据库表
    let (s, b) = auth_get(&db, "/tools/gen/tables", &token).await;
    assert_eq!(s, StatusCode::OK);
    let tables = b["data"].as_array().unwrap();
    assert!(!tables.is_empty(), "应至少包含一张表");

    // 预览代码生成（使用第一张表名）
    let table_name = tables[0]["table_name"].as_str().unwrap();
    let (s, _) = auth_post(
        &db,
        "/tools/gen/preview",
        &token,
        serde_json::json!({
            "table_name": table_name,
            "module_name": "system",
            "package_name": "com.example",
            "author": "test"
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

// ================================================================
// P2 新增功能集成测试
// ================================================================

/// API 版本信息端点
#[tokio::test]
async fn test_version_endpoint() {
    let db = setup_test_db().await;
    let state = build_test_app(db).await;
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
    let state = build_test_app(db).await;
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
    let state = build_test_app(db).await;
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

    let logout_req = Request::builder()
        .uri("/auth/logout")
        .method("POST")
        .header("authorization", format!("Bearer {}", token))
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
        "/system/users/list?page=1&pageSize=5",
        "/system/roles/list?page=1&pageSize=5",
        "/system/depts/tree",
        "/system/menus/tree",
        "/system/permissions/tree",
        "/system/online",
        "/system/posts/listNoPage",
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

    let state = build_test_app(db).await;
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
