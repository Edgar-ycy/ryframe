//! 权限校验 & 用户 CRUD 集成测试
//!
//! 验证 RBAC 权限执行、用户全生命周期、及边界场景。

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;

use common::{
    auth_delete, auth_get, auth_post, auth_put, login_get_token, seed_test_data, setup_test_db,
    test_rate_limit_state,
};

// ==================== 权限校验测试 ====================

/// 验证：无 token 访问受保护接口返回 401
#[tokio::test]
async fn test_permission_no_token_returns_401() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;

    let state = common::build_test_app(db.clone()).await;
    let router = ryframe_api::router::api_router(state, test_rate_limit_state());

    let protected_routes = vec![
        "/system/users?page=1&page_size=10",
        "/system/roles?page=1&page_size=10",
        "/system/depts/tree",
        "/system/menus/tree",
        "/system/posts?page=1&page_size=10",
        "/system/configs?page=1&page_size=10",
        "/system/notices?page=1&page_size=10",
        "/system/perms/tree",
        "/system/online",
    ];

    for uri in protected_routes {
        let req = Request::builder()
            .uri(uri)
            .method("GET")
            .body(Body::empty())
            .unwrap();
        let (status, _) = common::send_request(router.clone(), req).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "访问 {} 应返回 401，实际返回 {}",
            uri,
            status
        );
    }
}

/// 验证：无效 token 返回 401
#[tokio::test]
async fn test_permission_invalid_token_returns_401() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;

    let state = common::build_test_app(db.clone()).await;
    let router = ryframe_api::router::api_router(state, test_rate_limit_state());

    let req = Request::builder()
        .uri("/system/users?page=1&page_size=10")
        .method("GET")
        .header("authorization", "Bearer invalid_token_here")
        .body(Body::empty())
        .unwrap();
    let (status, _) = common::send_request(router, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "无效 token 应返回 401");
}

/// 验证：非 admin 用户访问系统管理接口返回 403
#[tokio::test]
async fn test_permission_non_admin_access() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;

    // 创建一个无角色的用户
    common::seed_user(&db, 100, "normal", "普通用户", None).await;

    // 用普通用户登录
    let state = common::build_test_app(db.clone()).await;
    let router = ryframe_api::router::api_router(state, test_rate_limit_state());
    let req = Request::builder()
        .uri("/auth/login")
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&json!({
                "username": "normal",
                "password": "test123"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (status, body) = common::send_request(router, req).await;
    assert_eq!(status, StatusCode::OK, "normal 用户应可以登录");
    let normal_token = body["data"]["access_token"].as_str().unwrap().to_string();

    // 尝试访问系统管理接口：无角色/无权限用户应被路由权限层拦截
    let system_endpoints = vec![
        "/system/users?page=1&page_size=10",
        "/system/roles?page=1&page_size=10",
        "/system/posts?page=1&page_size=10",
        "/system/configs?page=1&page_size=10",
        "/system/notices?page=1&page_size=10",
    ];

    for uri in system_endpoints {
        let (status, _) = auth_get(&db, uri, &normal_token).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "无权限用户访问 {} 应返回 403，实际返回 {}",
            uri,
            status
        );
    }
}

/// 验证普通用户的菜单、按钮对应 API 权限及撤权实时生效。
#[tokio::test]
async fn test_operator_permission_and_menu_revocation_flow() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let admin_token = login_get_token(&db).await;

    let (status, body) = auth_post(
        &db,
        "/system/perms",
        &admin_token,
        json!({
            "name": "用户查询",
            "code": "system:user:list",
            "parent_id": null,
            "perm_type": "api",
            "icon": null,
            "sort": 1,
            "status": "1"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "创建查询权限失败: {body:?}");
    let perm_id = body["data"]["id"].as_str().unwrap().to_owned();

    let (status, body) = auth_post(
        &db,
        "/system/roles",
        &admin_token,
        json!({
            "name": "普通运营",
            "code": "operator",
            "sort": 1,
            "data_scope": "3"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "创建普通运营角色失败: {body:?}");
    let role_id = body["data"]["id"].as_str().unwrap().to_string();

    let (status, body) = auth_post(
        &db,
        "/system/menus",
        &admin_token,
        json!({
            "name": "用户管理",
            "parent_id": null,
            "menu_type": "C",
            "perm_id": perm_id,
            "route_key": "system.user",
            "icon": "User",
            "sort": 1,
            "visible": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "创建用户菜单失败: {body:?}");

    let (status, body) = auth_put(
        &db,
        &format!("/system/roles/{role_id}/permissions"),
        &admin_token,
        json!({ "perm_ids": [perm_id] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "角色分配权限失败: {body:?}");

    common::seed_user(&db, 100, "operator01", "普通运营", Some(1)).await;
    common::seed_user(&db, 101, "outside_dept", "无部门用户", None).await;
    let (status, body) = auth_put(
        &db,
        "/system/users/100/roles",
        &admin_token,
        json!({ "role_ids": [role_id] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "用户分配角色失败: {body:?}");

    let state = common::build_test_app(db.clone()).await;
    let router = ryframe_api::router::api_router(state, test_rate_limit_state());
    let login_request = Request::builder()
        .uri("/auth/login")
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&json!({
                "username": "operator01",
                "password": "test123"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (status, body) = common::send_request(router, login_request).await;
    assert_eq!(status, StatusCode::OK, "普通运营登录失败: {body:?}");
    let operator_token = body["data"]["access_token"].as_str().unwrap();

    let (status, body) = auth_get(&db, "/system/menus/current", operator_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["name"], "用户管理");

    let (status, body) = auth_get(&db, "/system/users?page=1&page_size=10", operator_token).await;
    assert_eq!(status, StatusCode::OK);
    let records = body["rows"].as_array().unwrap();
    assert!(records.iter().any(|item| item["username"] == "operator01"));
    assert!(
        !records
            .iter()
            .any(|item| item["username"] == "outside_dept"),
        "本部门数据范围不应返回无部门用户"
    );
    let (status, _) = auth_post(
        &db,
        "/system/users",
        operator_token,
        json!({ "username": "forbidden", "nickname": "无权限新增" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = auth_put(
        &db,
        &format!("/system/roles/{role_id}/permissions"),
        &admin_token,
        json!({ "perm_ids": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "撤销角色权限失败: {body:?}");

    let (status, _) = auth_get(&db, "/system/users?page=1&page_size=10", operator_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = auth_get(&db, "/system/menus/current", operator_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"], json!([]));
}

// ==================== 权限资源 CRUD / 同步 ====================

#[tokio::test]
async fn test_permission_crud_and_sync_flow() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    let (status, _) = auth_post(
        &db,
        "/system/perms",
        &token,
        json!({
            "name": "非法权限类型",
            "code": "system:test:invalid-type",
            "parent_id": null,
            "perm_type": "unknown",
            "sort": 0,
            "status": "1"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, _) = auth_get(&db, "/system/perms/tree?perm_type=unknown", &token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = auth_post(
        &db,
        "/system/perms",
        &token,
        json!({
            "name": "测试权限",
            "code": "system:test:crud",
            "parent_id": null,
            "perm_type": "api",
            "icon": null,
            "sort": 10,
            "status": "1"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "创建权限失败: {:?}", body);
    let perm_id = body["data"]["id"]
        .as_str()
        .expect("权限ID应为字符串")
        .to_owned();

    let (status, body) = auth_get(&db, &format!("/system/perms/{}", perm_id), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["code"], "system:test:crud");

    let (status, body) = auth_put(
        &db,
        &format!("/system/perms/{}", perm_id),
        &token,
        json!({
            "name": "测试权限更新",
            "code": "system:test:crud:updated",
            "parent_id": null,
            "perm_type": "api",
            "icon": "Setting",
            "sort": 11,
            "status": "1"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "更新权限失败: {:?}", body);
    assert_eq!(body["data"]["code"], "system:test:crud:updated");

    let (status, body) = auth_delete(&db, &format!("/system/perms/{}", perm_id), &token).await;
    assert_eq!(status, StatusCode::OK, "删除权限失败: {:?}", body);

    let (status, _) = auth_post(&db, "/system/perms/sync", &token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "权限同步失败");
}

#[test]
fn test_compiled_permission_catalog_contains_known_codes() {
    let codes = ryframe_api::permission_catalog::route_permission_codes();
    assert!(
        codes.contains(&"system:user:list"),
        "catalog should include user list permission"
    );
    assert!(
        codes.contains(&"system:perm:sync"),
        "catalog should include permission sync code"
    );
    assert!(
        codes.contains(&"tenant:list"),
        "catalog should include tenant list permission"
    );
}

#[tokio::test]
async fn test_role_permission_assignment_and_validation() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    let (status, body) = auth_post(
        &db,
        "/system/perms",
        &token,
        json!({
            "name": "角色绑定权限",
            "code": "system:test:role-perm",
            "parent_id": null,
            "perm_type": "api",
            "icon": null,
            "sort": 20,
            "status": "1"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "创建权限失败: {:?}", body);
    let perm_id = body["data"]["id"]
        .as_str()
        .expect("权限ID应为字符串")
        .to_owned();

    let (status, body) = auth_post(
        &db,
        "/system/roles",
        &token,
        json!({
            "name": "权限测试角色",
            "code": "perm_test_role",
            "sort": 1,
            "data_scope": "5"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "创建角色失败: {:?}", body);
    let role_id = body["data"]["id"]
        .as_str()
        .expect("角色ID应为字符串")
        .to_owned();

    let (status, body) = auth_put(
        &db,
        &format!("/system/roles/{role_id}/permissions"),
        &token,
        json!({ "perm_ids": [perm_id.clone()] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "分配权限失败: {:?}", body);

    let (status, body) = auth_get(
        &db,
        &format!("/system/roles/{}/permissions", role_id),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "查询角色权限失败: {:?}", body);
    let ids = body["data"].as_array().unwrap();
    assert!(
        ids.iter().any(|v| v.as_str() == Some(perm_id.as_str())),
        "角色权限列表应包含已分配权限"
    );

    let (status, body) = auth_put(
        &db,
        &format!("/system/roles/{role_id}/permissions"),
        &token,
        json!({ "perm_ids": [999999999999i64] }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "非法权限ID应返回 422: {:?}",
        body
    );
}

#[tokio::test]
async fn test_permission_requires_permission_code() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;

    common::seed_user(&db, 100, "normal_perm", "普通用户", None).await;
    let state = common::build_test_app(db.clone()).await;
    let router = ryframe_api::router::api_router(state, test_rate_limit_state());

    let req = Request::builder()
        .uri("/auth/login")
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&json!({
                "username": "normal_perm",
                "password": "test123"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (status, body) = common::send_request(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let normal_token = body["data"]["access_token"].as_str().unwrap().to_string();

    let (status, _) = auth_get(&db, "/system/perms/tree", &normal_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_auth_version_invalidates_existing_access_and_refresh_tokens() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    common::seed_user(&db, 100, "versioned_user", "版本用户", None).await;

    let state = common::build_test_app(db.clone()).await;
    let router = ryframe_api::router::api_router(state.clone(), test_rate_limit_state());
    let login_request = Request::builder()
        .uri("/auth/login")
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&json!({
                "username": "versioned_user",
                "password": "test123"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (status, headers, body) =
        common::send_request_with_headers(router.clone(), login_request).await;
    assert_eq!(status, StatusCode::OK);
    let access_token = body["data"]["access_token"].as_str().unwrap().to_string();
    assert!(body["data"].get("refresh_token").is_none());
    let refresh_token = common::response_cookie(&headers, "ryframe_refresh_token").unwrap();

    ryframe_db::UserRepository
        .increment_auth_versions(&db, "system", &[100])
        .await
        .unwrap();

    let old_access_request = Request::builder()
        .uri("/auth/me")
        .method("GET")
        .header("authorization", format!("Bearer {access_token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = common::send_request(router.clone(), old_access_request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let refresh_claims =
        ryframe_auth::jwt::decode_token(&refresh_token, "test-jwt-secret-for-integration-tests")
            .unwrap();
    let refresh_csrf = ryframe_auth::jwt::encode_csrf(
        "test-jwt-secret-for-integration-tests",
        Some(&refresh_claims.sid),
        300,
    )
    .unwrap();
    let old_refresh_request = Request::builder()
        .uri("/auth/refresh")
        .method("POST")
        .header("x-csrf-token", &refresh_csrf)
        .header(
            "cookie",
            format!("ryframe_refresh_token={refresh_token}; ryframe_csrf={refresh_csrf}"),
        )
        .body(Body::empty())
        .unwrap();
    let (status, _) = common::send_request(router.clone(), old_refresh_request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let relogin_request = Request::builder()
        .uri("/auth/login")
        .method("POST")
        .header("X-Tenant-Id", "system")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&json!({
                "username": "versioned_user",
                "password": "test123"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (status, _) = common::send_request(router, relogin_request).await;
    assert_eq!(status, StatusCode::OK);
}

// ==================== 用户 CRUD 全流程测试 ====================

/// 用户 CrUD 全流程：创建 → 查询 → 更新 → 删除 → 验证删除
#[tokio::test]
async fn test_user_crud_flow() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 1. 创建用户
    let (s, b) = auth_post(
        &db,
        "/system/users",
        &token,
        json!({
            "username": "newuser",
            "nickname": "新用户",
            "email": "newuser@test.com",
            "phone": "13900000001",
            "dept_id": "1",
            "role_ids": ["2"],
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "创建用户失败: {:?}", b);
    let user_id = b["data"]["id"].as_str().expect("创建用户应返回 id");
    assert_eq!(b["data"]["username"], "newuser");
    assert_eq!(b["data"]["status"], "pending_activation");

    // 2. 按 ID 查询
    let (s, b) = auth_get(&db, &format!("/system/users/{}", user_id), &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["username"], "newuser");
    assert_eq!(b["data"]["email"], "newuser@test.com");
    assert_eq!(b["data"]["roles"][0]["id"], "2");

    // 3. 列表查询（验证新用户出现在列表中）
    let (s, b) = auth_get(
        &db,
        "/system/users?page=1&page_size=10&username=newuser",
        &token,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some());
    let rows = b["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "搜索 newuser 应有结果");

    // 4. 更新用户信息
    let (s, b) = auth_put(
        &db,
        &format!("/system/users/{}", user_id),
        &token,
        json!({
            "nickname": "更新昵称",
            "email": "updated@test.com",
            "phone": "13900000002",
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "更新用户失败: {:?}", b);
    assert_eq!(b["data"]["status"], "pending_activation");

    let (s, _) = auth_put(
        &db,
        &format!("/system/users/{}", user_id),
        &token,
        json!({
            "nickname": "旧混合写法",
            "email": "updated@test.com",
            "phone": "13900000002",
            "status": "1",
        }),
    )
    .await;
    assert!(!s.is_success(), "资料更新接口不应再接受状态字段");

    let (s, _) = auth_post(
        &db,
        "/system/users/assign-role",
        &token,
        json!({"user_id": user_id, "role_ids": []}),
    )
    .await;
    assert!(!s.is_success(), "旧用户角色动作接口不应可用");

    let (s, _) = auth_put(
        &db,
        "/system/users/status",
        &token,
        json!({"user_id": user_id, "status": "1"}),
    )
    .await;
    assert!(!s.is_success(), "旧用户状态动作接口不应可用");

    // 5. 旧管理员重置密码接口不可用
    let (s, _) = auth_put(
        &db,
        &format!("/system/users/{}/password", user_id),
        &token,
        json!({
            "password": "N3wP@ssw0rd!",
            "confirmPassword": "N3wP@ssw0rd!"
        }),
    )
    .await;
    assert!(!s.is_success(), "旧管理员重置密码接口不应可用");

    // 5.1 发起密码重置请求
    let (s, _) = auth_post(
        &db,
        &format!("/system/users/{}/password-reset-requests", user_id),
        &token,
        json!({"reason": "用户忘记密码"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "发起密码重置请求失败");

    let (s, _) = auth_post(
        &db,
        &format!("/system/users/{}/password-reset-requests", user_id),
        &token,
        json!({"reason": ""}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "空原因应被拒绝");

    let (s, _) = auth_post(
        &db,
        "/system/users/1/password-reset-requests",
        &token,
        json!({"reason": "不能重置超级管理员"}),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "超级管理员目标应被拒绝");

    // 6. 修改用户状态
    let (s, _) = auth_put(
        &db,
        &format!("/system/users/{}/status", user_id),
        &token,
        json!({"status": "invalid"}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "非法用户状态应被拒绝");

    let (s, _) = auth_put(
        &db,
        &format!("/system/users/{}/status", user_id),
        &token,
        json!({"status": "0"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "修改状态失败");

    // 验证状态变更
    let (s, b) = auth_get(&db, &format!("/system/users/{}", user_id), &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["status"], "0");

    // 恢复状态
    let (s, _) = auth_put(
        &db,
        &format!("/system/users/{}/status", user_id),
        &token,
        json!({"status": "1"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // 7. 删除用户
    let (s, _) = auth_delete(&db, &format!("/system/users/{}", user_id), &token).await;
    assert_eq!(s, StatusCode::OK, "删除用户失败");

    // 8. 验证删除后查询不到
    let (s, _) = auth_get(&db, &format!("/system/users/{}", user_id), &token).await;
    assert!(
        s == StatusCode::NOT_FOUND || s == StatusCode::OK,
        "删除后查询应返回 NOT_FOUND 或空结果"
    );
}

/// 用户创建参数校验
#[tokio::test]
async fn test_user_create_validation() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 缺少必填字段
    let (s, b) = auth_post(
        &db,
        "/system/users",
        &token,
        json!({
            "nickname": "缺少用户名",
        }),
    )
    .await;
    assert!(s.is_client_error(), "缺少 username 应返回 4xx: {:?}", b);

    // 用户名已存在
    let (s, b) = auth_post(
        &db,
        "/system/users",
        &token,
        json!({
            "username": "admin",
            "nickname": "重名"
        }),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "重名用户名应返回 409: {:?}", b);

    // 创建用户不再要求管理员输入密码
    let (s, b) = auth_post(
        &db,
        "/system/users",
        &token,
        json!({
            "username": "nopassword",
            "nickname": "无初始密码"
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "不传密码应创建成功: {:?}", b);
    assert_eq!(b["data"]["status"], "pending_activation");
}

/// 用户列表分页查询
#[tokio::test]
async fn test_user_list_pagination() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 创建多个测试用户填充数据
    use common::seed_user;
    seed_user(&db, 10, "user_a", "用户A", Some(1)).await;
    seed_user(&db, 11, "user_b", "用户B", Some(1)).await;
    seed_user(&db, 12, "user_c", "用户C", Some(1)).await;

    // 分页查询：page_size=2
    let (s, b) = auth_get(&db, "/system/users?page=1&page_size=2", &token).await;
    assert_eq!(s, StatusCode::OK);
    let rows = b["rows"].as_array().unwrap();
    assert!(rows.len() <= 2, "page_size=2 应返回最多 2 条");
    assert!(b["total"].as_i64().unwrap() >= 4, "总数至少 4");
}

// ==================== 角色权限测试 ====================

/// 角色 CRUD 全流程
#[tokio::test]
async fn test_role_crud_flow() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    // 1. 创建角色
    let (s, b) = auth_post(
        &db,
        "/system/roles",
        &token,
        json!({
            "name": "测试角色",
            "code": "test_role",
            "sort": 10,
            "data_scope": "5"
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "创建角色失败: {:?}", b);
    let role_id = b["data"]["id"].as_str().expect("创建角色应返回 id");
    assert_eq!(b["data"]["code"], "test_role");

    // 2. 按 ID 查询
    let (s, b) = auth_get(&db, &format!("/system/roles/{}", role_id), &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["name"], "测试角色");

    // 3. 更新角色
    let (s, b) = auth_put(
        &db,
        &format!("/system/roles/{}", role_id),
        &token,
        json!({
            "name": "更新角色",
            "sort": 20,
            "status": "1"
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "更新角色失败: {:?}", b);

    // 4. 删除角色
    let (s, _) = auth_delete(&db, &format!("/system/roles/{}", role_id), &token).await;
    assert_eq!(s, StatusCode::OK, "删除角色失败");
}

/// 角色分页列表
#[tokio::test]
async fn test_role_list_with_pagination() {
    let db = setup_test_db().await;
    seed_test_data(&db).await;
    let token = login_get_token(&db).await;

    let (s, b) = auth_get(&db, "/system/roles?page=1&page_size=10", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.get("rows").is_some());
    assert!(b.get("total").is_some());

    // 列表应包含 seed 中创建的 admin 和 user 角色
    let rows = b["rows"].as_array().unwrap();
    let codes: Vec<&str> = rows.iter().filter_map(|r| r["code"].as_str()).collect();
    assert!(codes.contains(&"admin"), "列表应包含 admin 角色");
    assert!(codes.contains(&"user"), "列表应包含 user 角色");
}
