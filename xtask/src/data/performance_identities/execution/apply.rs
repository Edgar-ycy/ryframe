use super::{
    Context, IdentitySession, Ledger, array, close, permission_ids, request, same_set, text,
};
use crate::{
    Result,
    utc_timestamp::{now_order_key, rfc3339_order_key},
};
use serde_json::{Value, json};

const ADMIN: &[&str] = &[
    "system:role:list",
    "system:role:add",
    "system:role:edit",
    "system:perm:list",
    "system:user:list",
    "system:user:add",
    "system:user:edit",
];

pub(super) async fn run(
    plan: &Value,
    ledger: &mut Ledger,
    context: &mut dyn Context,
    before: &Value,
) -> Result<Value> {
    preflight(plan, ledger, context).await?;
    let checked = context.inspect("verify").await?;
    if ["scope_id", "server_uuid", "api_process"]
        .iter()
        .any(|key| checked[key] != before[key])
    {
        return Err("首个身份写入前 API 进程或资源绑定变化".into());
    }
    for group in array(&plan["groups"])? {
        let mut admin = context.session(&group["admin"]).await?;
        let result = async {
            admin.login().await?;
            let role = prepare_role(group, ledger, &mut *admin).await?;
            for user in array(&group["users"])? {
                prepare_user(plan, group, user, &role, ledger, &mut *admin, context).await?;
            }
            Ok(())
        }
        .await;
        close(&mut *admin, result).await?;
    }
    Ok(json!({}))
}
async fn preflight(plan: &Value, ledger: &Ledger, context: &mut dyn Context) -> Result<()> {
    for group in array(&plan["groups"])? {
        let mut admin = context.session(&group["admin"]).await?;
        let result = async {
            admin.login().await?;
            let auth = request(&mut *admin, "get_auth_context", Value::Null, Value::Null).await?;
            let authorized = auth["is_super_admin"] == true
                || ADMIN.iter().all(|code| {
                    auth["permissions"]
                        .as_array()
                        .is_some_and(|values| values.contains(&json!(code)))
                });
            if auth["user"]["tenant_id"] != group["tenant_id"]
                || auth["user"]["username"] != group["admin"]["username"]
                || auth["password_change_required"] != false
                || !authorized
            {
                return Err("准备管理员身份、首次改密状态或必要权限不符".into());
            }
            let role_query = json!({"code":group["role_code"],"page":1,"page_size":100});
            let prefix = format!("dv-{}-", text(&plan["environment"], "plan_id")?);
            let user_query = json!({"username":prefix,"page":1,"page_size":100});
            let roles = request(&mut *admin, "get_system_roles", Value::Null, role_query).await?;
            let users = request(&mut *admin, "get_system_users", Value::Null, user_query).await?;
            let slot = text(group, "slot")?;
            let role_count = u64::from(ledger.state["roles"].get(slot).is_some());
            let user_count = array(&group["users"])?
                .iter()
                .filter(|user| {
                    user["user_slot"]
                        .as_str()
                        .is_some_and(|slot| ledger.state["users"].get(slot).is_some())
                })
                .count();
            if roles["total"].as_u64() != Some(role_count)
                || users["total"].as_u64() != Some(user_count as u64)
            {
                return Err("同名对象与账本已确认前缀不同，拒绝接管".into());
            }
            let tree = request(
                &mut *admin,
                "get_system_perms_tree",
                Value::Null,
                Value::Null,
            )
            .await?;
            permission_ids(&tree, &group["permissions"])?;
            Ok(())
        }
        .await;
        close(&mut *admin, result).await?;
    }
    Ok(())
}
pub(super) fn role(value: &Value, group: &Value, id: &str) -> Result<()> {
    if value["id"] != id
        || value["code"] != group["role_code"]
        || value["is_super"] != 0
        || value["status"] != "1"
        || value["data_scope"] != "1"
    {
        return Err("已登记角色状态或归属发生变化".into());
    }
    Ok(())
}
pub(super) fn administrative_user_detail(
    value: &Value,
    user: &Value,
    role: &str,
    id: &str,
) -> Result<()> {
    if value["id"] != id
        || value["username"] != user["username"]
        || value["status"] != "1"
        || contradictory_password_flag(value, false)
        || !value["roles"].as_array().is_some_and(|roles| {
            roles.len() == 1 && roles[0]["id"] == role && roles[0]["is_super"] == 0
        })
    {
        return Err("已登记用户状态、首次改密要求或角色发生变化".into());
    }
    Ok(())
}

fn contradictory_password_flag(value: &Value, required: bool) -> bool {
    value
        .get("password_change_required")
        .is_some_and(|flag| flag.as_bool() != Some(required))
}

async fn prepare_role(
    group: &Value,
    ledger: &mut Ledger,
    admin: &mut dyn IdentitySession,
) -> Result<String> {
    let slot = text(group, "slot")?;
    let permissions = permission_ids(
        &request(admin, "get_system_perms_tree", Value::Null, Value::Null).await?,
        &group["permissions"],
    )?;
    let entry = ledger.take("post_system_roles", slot)?;
    if entry.is_none() {
        let step = json!({"operation":"post_system_roles","body":{
            "code":group["role_code"],"name":"运行时性能普通用户","sort":0,"data_scope":"1"
        }});
        mutation(
            ledger,
            admin,
            "post_system_roles",
            slot,
            step,
            async |response| {
                let id = super::super::state::id(&response["data"]["id"])?;
                role(&response["data"], group, id)?;
                Ok(json!({"id":id}))
            },
        )
        .await?;
    }
    let id = text(&ledger.state["roles"], slot)?.to_owned();
    role(
        &request(
            admin,
            "get_system_roles_by_id",
            json!({"id":id}),
            Value::Null,
        )
        .await?,
        group,
        &id,
    )?;
    if ledger
        .take("put_system_roles_by_id_permissions", slot)?
        .is_none()
    {
        let step = json!({"operation":"put_system_roles_by_id_permissions",
            "path":{"id":id},"body":{"perm_ids":permissions}});
        mutation(
            ledger,
            admin,
            "put_system_roles_by_id_permissions",
            slot,
            step,
            async |_| Ok(json!({"id":id})),
        )
        .await?;
    }
    let actual = request(
        admin,
        "get_system_roles_by_id_permissions",
        json!({"id":id}),
        Value::Null,
    )
    .await?;
    if !same_set(&actual, &permissions) {
        return Err("角色分配后的权限与计划不同".into());
    }
    Ok(id)
}
async fn create_user(
    group: &Value,
    user: &Value,
    role_id: &str,
    ledger: &mut Ledger,
    admin: &mut dyn IdentitySession,
    context: &mut dyn Context,
) -> Result<bool> {
    let slot = text(user, "user_slot")?;
    if ledger.take("post_system_users", slot)?.is_some() {
        return Ok(true);
    }
    let step = json!({"operation":"post_system_users","body":{
        "username":user["username"],"nickname":"运行时性能普通用户","role_ids":[role_id]
    }});
    mutation(
        ledger,
        admin,
        "post_system_users",
        slot,
        step,
        async |response| {
            let value = &response["data"];
            let id = super::super::state::id(&value["id"])?;
            if value["username"] != user["username"]
                || value["status"] != "1"
                || contradictory_password_flag(value, true)
            {
                return Err("新用户响应的身份或启用、首次改密状态与计划不符".into());
            }
            context
                .observe_created_user(group, user, role_id, id)
                .await?;
            Ok(json!({"id":id}))
        },
    )
    .await?;
    Ok(false)
}

async fn prepare_user(
    plan: &Value,
    group: &Value,
    user: &Value,
    role_id: &str,
    ledger: &mut Ledger,
    admin: &mut dyn IdentitySession,
    context: &mut dyn Context,
) -> Result<()> {
    let already_created = create_user(group, user, role_id, ledger, admin, context).await?;
    let slot = text(user, "user_slot")?;
    let id = text(&ledger.state["users"], slot)?.to_owned();
    let mut secret = None;
    if ledger
        .take("post_system_users_by_id_password_reset_requests", slot)?
        .is_none()
    {
        // 已确认创建的后继仍先读取真实首次改密状态，再申请重置。
        if already_created {
            context
                .observe_created_user(group, user, role_id, &id)
                .await?;
        }
        let step = json!({"operation":"post_system_users_by_id_password_reset_requests",
            "path":{"id":id},"body":{"reason":"本计划隔离性能普通用户首次改密"}});
        let response = mutation(
            ledger,
            admin,
            "post_system_users_by_id_password_reset_requests",
            slot,
            step,
            async |response| {
                let activation = activation(response, &plan["environment"], group)?;
                Ok(json!({"id":id,"request_id":activation["request_id"]}))
            },
        )
        .await?;
        secret = Some(activation(&response, &plan["environment"], group)?);
    }
    if ledger
        .take("post_auth_password_reset_complete", slot)?
        .is_none()
    {
        let mut body =
            secret.ok_or("密码请求已确认但激活秘密未落盘；需要人工核对，禁止重发或重放")?;
        body["new_password"] = json!(context.password(text(user, "credential_slot")?).await?);
        let mut identity = user.clone();
        identity["tenant_id"] = group["tenant_id"].clone();
        let mut session = context.session(&identity).await?;
        let result = async {
            request(&mut *session, "get_auth_csrf", Value::Null, Value::Null).await?;
            mutation(
                ledger,
                &mut *session,
                "post_auth_password_reset_complete",
                slot,
                json!({"operation":"post_auth_password_reset_complete","body":body}),
                async |_| Ok(json!({"id":id})),
            )
            .await?;
            Ok(())
        }
        .await;
        close(&mut *session, result).await?;
    }
    let detail = request(
        admin,
        "get_system_users_by_id",
        json!({"id":id}),
        Value::Null,
    )
    .await?;
    administrative_user_detail(&detail, user, role_id, &id)?;
    context
        .observe_activated_user(group, user, role_id, &id)
        .await
}
async fn mutation(
    ledger: &mut Ledger,
    session: &mut dyn IdentitySession,
    operation: &str,
    slot: &str,
    step: Value,
    confirm: impl AsyncFnOnce(&Value) -> Result<Value>,
) -> Result<Value> {
    ledger.started(operation, slot)?;
    let result = async {
        let response = session.request(&step).await?;
        let fields = confirm(&response).await?;
        ledger.confirmed(operation, slot, fields)?;
        Ok(response)
    }
    .await;
    if result.is_err() {
        ledger.unknown()?;
        return Err("写操作结果未确认，账本需要人工核对；禁止重放或按同名接管".into());
    }
    result
}
pub(super) fn activation(response: &Value, environment: &Value, group: &Value) -> Result<Value> {
    let data = &response["data"];
    let id = super::super::state::id(&data["request_id"])?;
    let base = url::Url::parse(text(environment, "frontend_url")?)?;
    let url = base.join(text(data, "reset_url")?)?;
    let pairs = url
        .fragment()
        .map(|value| {
            url::form_urlencoded::parse(value.as_bytes())
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect::<Vec<_>>()
        })
        .ok_or("激活地址缺少 fragment")?;
    let fields = pairs
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let get = |key| {
        pairs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    };
    let future =
        rfc3339_order_key(text(data, "expires_at")?).is_ok_and(|value| value > now_order_key());
    if url.origin() != base.origin()
        || url.path() != "/reset-password"
        || url.query().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || pairs.len() != 3
        || fields != std::collections::BTreeSet::from(["request_id", "tenant_id", "token"])
        || get("request_id") != Some(id)
        || get("tenant_id") != group["tenant_id"].as_str()
        || get("token").is_none_or(str::is_empty)
        || !future
    {
        return Err("激活 fragment、租户、请求或有效期与当前用户不同".into());
    }
    Ok(json!({"request_id":id,"tenant_id":group["tenant_id"],"token":get("token")}))
}
