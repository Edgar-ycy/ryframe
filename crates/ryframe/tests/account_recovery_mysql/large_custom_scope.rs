use std::collections::BTreeMap;

use axum::{Router, middleware::from_fn_with_state};
use ryframe_api::{
    auth_middleware::auth_middleware,
    generated::generated_router,
    handlers::{
        authorization_diagnostic_handler::authorization_diagnostic_router,
        role_handler::role_router,
    },
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};

use super::super::{
    NEW_PASSWORD,
    account_http::{Response, Server},
    initialize, message, mysql, sql_initialization,
};

const FIRST_DEPARTMENT: i64 = 90_001;
const DEPARTMENT_COUNT: i64 = 4_096;
const LAST_DEPARTMENT: i64 = FIRST_DEPARTMENT + DEPARTMENT_COUNT - 1;
const OUTSIDE_DEPARTMENT: i64 = LAST_DEPARTMENT + 1;
const ROLE: i64 = 60_001;
const EDITOR: i64 = 60_011;
const FIRST_OWNER: i64 = 60_021;
const LAST_OWNER: i64 = 60_022;
const OUTSIDE_OWNER: i64 = 60_023;
const FIRST_NOTICE: i64 = 91_001;
const LAST_NOTICE: i64 = 91_002;
const OUTSIDE_NOTICE: i64 = 91_003;
const FOREIGN_NOTICE: i64 = 91_004;
const OWNERLESS_NOTICE: i64 = 91_005;
const NOTICES: &str = "/api/v1/system/notices";
const UPDATED_TITLE: &str = "Scope4096-updated";
const UPDATED_CONTENT: &str = "真实自定义范围更新";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_scope_with_4096_departments_is_enforced_through_notice_http() {
    initialize();
    mysql::run_mysql_test("notice-large-scope", |database| async move {
        let exported = sql_initialization::import(&database).await?;
        let departments: Vec<_> = (FIRST_DEPARTMENT..=LAST_DEPARTMENT).collect();
        seed(&database, &departments).await?;
        let (state, _lifetime) = super::state(&database).await?;
        let system = state
            .services
            .identity
            .auth
            .login("system", "admin", sql_initialization::ADMIN_PASSWORD)
            .await
            .map_err(message)?;
        assert!(system.user_info.is_super_admin);
        let routes = Router::new().nest(
            "/api/v1/system",
            Router::new()
                .nest("/roles", role_router(state.clone()))
                .nest(
                    "/authorization-diagnostics",
                    authorization_diagnostic_router(state.clone()),
                )
                .merge(generated_router(
                    state.clone(),
                    &state.services.content.generated,
                    state.settings.pagination,
                )),
        );
        let server = Server::start_router(
            routes.layer(from_fn_with_state(state.auth.clone(), auth_middleware)),
        )
        .await?;
        grant_departments(&database, &server, &system.access_token, &departments).await?;

        let editor = state
            .services
            .identity
            .auth
            .login("system", "scope-4096-editor", NEW_PASSWORD)
            .await
            .map_err(message)?;
        assert!(!editor.user_info.is_super_admin);
        let mut permissions: Vec<_> = editor.user_info.perms.iter().map(String::as_str).collect();
        permissions.sort_unstable();
        assert_eq!(
            permissions,
            [
                "system:notice:edit",
                "system:notice:list",
                "system:notice:remove"
            ]
        );
        verify_list(&server, &editor.access_token, &[FIRST_NOTICE, LAST_NOTICE]).await?;
        verify_details(&server, &editor.access_token).await?;
        verify_writes(&database, &server, &editor.access_token).await?;
        drop(server);
        exported.finish()
    })
    .await;
}

async fn seed(database: &DatabaseConnection, departments: &[i64]) -> Result<(), String> {
    mysql::execute(
        database,
        "INSERT INTO sys_tenant(id,tenant_id,name) VALUES (60000,'scope-foreign','范围隔离租户')",
    )
    .await?;
    mysql::execute(database, &format!(
        "INSERT INTO sys_role(id,tenant_id,name,code,data_scope) VALUES ({ROLE},'system','自定义部门角色','scope-4096','2')"
    )).await?;
    mysql::execute(
        database,
        &format!(
            "INSERT INTO sys_role_permission(tenant_id,role_id,perm_id)
         SELECT 'system',{ROLE},id FROM sys_permission WHERE tenant_id='system'
           AND code IN ('system:notice:list','system:notice:edit','system:notice:remove')"
        ),
    )
    .await?;
    for batch in departments.chunks(256) {
        let values = batch
            .iter()
            .map(|id| format!("({id},'system','范围部门-{id}')"))
            .collect::<Vec<_>>()
            .join(",");
        mysql::execute(
            database,
            &format!("INSERT INTO sys_dept(id,tenant_id,name) VALUES {values}"),
        )
        .await?;
    }
    mysql::execute(database, &format!(
        "INSERT INTO sys_dept(id,tenant_id,name) VALUES ({OUTSIDE_DEPARTMENT},'system','授权外部门')"
    )).await?;
    seed_accounts(database).await?;
    for (id, tenant, owner) in [
        (FIRST_NOTICE, "system", Some(FIRST_OWNER)),
        (LAST_NOTICE, "system", Some(LAST_OWNER)),
        (OUTSIDE_NOTICE, "system", Some(OUTSIDE_OWNER)),
        // 同一允许部门的创建人，单独验证公告自身的租户过滤。
        (FOREIGN_NOTICE, "scope-foreign", Some(FIRST_OWNER)),
        (OWNERLESS_NOTICE, "system", None),
    ] {
        database.execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO sys_notice(id,tenant_id,title,content,status,created_by) VALUES (?,?,'Scope4096-original','初始正文','1',?)",
            [id.into(), tenant.into(), owner.into()],
        )).await.map_err(message)?;
    }
    Ok(())
}

async fn seed_accounts(database: &DatabaseConnection) -> Result<(), String> {
    let password_hash = ryframe_auth::password::hash(NEW_PASSWORD).map_err(message)?;
    for (id, username, department) in [
        (EDITOR, "scope-4096-editor", OUTSIDE_DEPARTMENT),
        (FIRST_OWNER, "scope-4096-first", FIRST_DEPARTMENT),
        (LAST_OWNER, "scope-4096-last", LAST_DEPARTMENT),
        (OUTSIDE_OWNER, "scope-4096-outside", OUTSIDE_DEPARTMENT),
    ] {
        database.execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO sys_user(id,tenant_id,username,password_hash,nickname,dept_id) VALUES (?,'system',?,?,'范围验收',?)",
            [id.into(), username.into(), password_hash.clone().into(), department.into()],
        )).await.map_err(message)?;
    }
    mysql::execute(
        database,
        &format!(
            "INSERT INTO sys_user_role(tenant_id,user_id,role_id) VALUES ('system',{EDITOR},{ROLE})"
        ),
    )
    .await?;
    mysql::execute(
        database,
        "UPDATE sys_user SET password_change_required=0 WHERE tenant_id='system' AND id=1",
    )
    .await
}

async fn grant_departments(
    database: &DatabaseConnection,
    server: &Server,
    admin: &str,
    departments: &[i64],
) -> Result<(), String> {
    let ids = departments
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let response = server
        .request(
            "PUT",
            &format!("/api/v1/system/roles/{ROLE}/data-scope"),
            admin,
            "Content-Type: application/json\r\n",
            serde_json::to_vec(&json!({ "data_scope": "2", "dept_ids": ids })).map_err(message)?,
        )
        .await?;
    assert_eq!(response.status, 200, "角色自定义部门授权失败");
    let saved = database.query_all_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT dept_id FROM sys_role_dept WHERE tenant_id='system' AND role_id=? ORDER BY dept_id",
        [ROLE.into()],
    )).await.map_err(message)?;
    let saved = saved
        .iter()
        .map(|row| row.try_get("", "dept_id").map_err(message))
        .collect::<Result<Vec<i64>, _>>()?;
    assert_eq!(saved.len(), departments.len());
    assert!(
        saved.as_slice() == departments,
        "数据库的角色部门集合与授权请求不完全一致"
    );
    let response = server
        .get(
            &format!("/api/v1/system/authorization-diagnostics/users/{EDITOR}"),
            admin,
        )
        .await?;
    assert_eq!(response.status, 200, "无法读取真实账号的授权投影");
    let scope = &response.body["data"]["data_scope"];
    assert_eq!(scope["scope"], "custom");
    assert_eq!(scope["include_self"], false);
    let resolved = response_ids(&scope["custom_departments"])?;
    assert_eq!(resolved.len(), departments.len());
    assert!(
        resolved.as_slice() == departments,
        "账号解析后的部门集合与数据库授权不完全一致"
    );
    Ok(())
}

fn response_ids(items: &Value) -> Result<Vec<i64>, String> {
    let mut ids = items
        .as_array()
        .ok_or("响应缺少记录数组")?
        .iter()
        .map(|item| {
            item["id"]
                .as_str()
                .ok_or("记录缺少字符串 ID")?
                .parse()
                .map_err(message)
        })
        .collect::<Result<Vec<_>, String>>()?;
    ids.sort_unstable();
    Ok(ids)
}

async fn verify_list(server: &Server, token: &str, expected: &[i64]) -> Result<(), String> {
    let response = server
        .get(
            &format!("{NOTICES}?title=Scope4096&page=1&page_size=10"),
            token,
        )
        .await?;
    assert_eq!(response.status, 200, "已授权账号列表请求失败");
    assert_eq!(response.body["data"]["total"], expected.len());
    assert_eq!(response_ids(&response.body["data"]["items"])?, expected);
    Ok(())
}

async fn verify_details(server: &Server, token: &str) -> Result<(), String> {
    for id in [FIRST_NOTICE, LAST_NOTICE] {
        let response = server.get(&format!("{NOTICES}/{id}"), token).await?;
        assert_eq!(response.status, 200, "允许的公告 {id} 无法读取");
        assert_eq!(response.body["data"]["id"], id.to_string());
        assert_eq!(response.body["data"]["revision"], "1");
    }
    for id in [OUTSIDE_NOTICE, FOREIGN_NOTICE, OWNERLESS_NOTICE] {
        let response = server.get(&format!("{NOTICES}/{id}"), token).await?;
        assert_eq!(response.status, 404, "越界公告 {id} 被详情接口暴露");
    }
    Ok(())
}

async fn verify_writes(
    database: &DatabaseConnection,
    server: &Server,
    token: &str,
) -> Result<(), String> {
    let before = images(database).await?;
    assert_eq!(before.len(), 5);
    let response = update(server, token, FIRST_NOTICE).await?;
    assert_eq!(response.status, 200);
    assert_eq!(response.body["data"]["revision"], "2");
    let updated = images(database).await?;
    let mut expected = before.clone();
    let image = expected
        .get_mut(&FIRST_NOTICE)
        .ok_or("缺少允许更新的公告前像")?;
    image["title"] = json!(UPDATED_TITLE);
    image["content"] = json!(UPDATED_CONTENT);
    image["revision"] = json!(2);
    image["updated_at"] = updated[&FIRST_NOTICE]["updated_at"].clone();
    assert_eq!(updated, expected, "更新改变了目标以外的字段或公告");

    let response = server
        .request(
            "DELETE",
            &format!("{NOTICES}/{LAST_NOTICE}"),
            token,
            "",
            Vec::new(),
        )
        .await?;
    assert_eq!(response.status, 200);
    let deleted = images(database).await?;
    let image = expected
        .get_mut(&LAST_NOTICE)
        .ok_or("缺少允许软删的公告前像")?;
    image["del_flag"] = json!("2");
    image["updated_at"] = deleted[&LAST_NOTICE]["updated_at"].clone();
    assert_eq!(deleted, expected, "软删改变了目标以外的字段或公告");
    assert_eq!(
        server
            .get(&format!("{NOTICES}/{LAST_NOTICE}"), token)
            .await?
            .status,
        404
    );
    verify_list(server, token, &[FIRST_NOTICE]).await?;

    for id in [OUTSIDE_NOTICE, FOREIGN_NOTICE, OWNERLESS_NOTICE] {
        assert_eq!(
            update(server, token, id).await?.status,
            404,
            "越界更新 {id}"
        );
        let response = server
            .request("DELETE", &format!("{NOTICES}/{id}"), token, "", Vec::new())
            .await?;
        assert_eq!(response.status, 404, "越界删除 {id}");
        assert_eq!(images(database).await?, deleted, "拒绝写入后原公告前像漂移");
    }
    Ok(())
}

async fn update(server: &Server, token: &str, id: i64) -> Result<Response, String> {
    let body = json!({
        "expected_revision": "1",
        "title": UPDATED_TITLE,
        "content_markdown": UPDATED_CONTENT,
        "status": "1"
    });
    server
        .request(
            "PUT",
            &format!("{NOTICES}/{id}"),
            token,
            "Content-Type: application/json\r\n",
            serde_json::to_vec(&body).map_err(message)?,
        )
        .await
}

async fn images(database: &DatabaseConnection) -> Result<BTreeMap<i64, Value>, String> {
    let rows = database.query_all_raw(Statement::from_string(
        DbBackend::MySql,
        "SELECT id, CAST(JSON_OBJECT('id',id,'tenant_id',tenant_id,'title',title,'content',content,
           'notice_type',`type`,'status',status,'created_by',created_by,'del_flag',del_flag,
           'created_at',created_at,'updated_at',updated_at,'revision',revision) AS CHAR) AS image
         FROM sys_notice",
    )).await.map_err(message)?;
    rows.into_iter()
        .map(|row| {
            let id = row.try_get("", "id").map_err(message)?;
            let image: String = row.try_get("", "image").map_err(message)?;
            Ok((id, serde_json::from_str(&image).map_err(message)?))
        })
        .collect()
}
