use super::{
    Context, IdentitySession, Ledger, apply, array, close, permission_ids, receipt, request,
    same_set, text,
};
use crate::Result;
use serde_json::{Value, json};

pub(super) async fn run(
    plan: &Value,
    ledger: &mut Ledger,
    context: &mut dyn Context,
) -> Result<Value> {
    let mut templates = Vec::with_capacity(10);
    for group in array(&plan["groups"])? {
        verify_admin(group, ledger, context).await?;
        for (index, user) in array(&group["users"])?.iter().enumerate() {
            let id = text(&ledger.state["users"], text(user, "user_slot")?)?;
            let mut identity = user.clone();
            identity["tenant_id"] = group["tenant_id"].clone();
            let mut session = context.session(&identity).await?;
            let result = async {
                session.login().await?;
                let auth =
                    request(&mut *session, "get_auth_context", Value::Null, Value::Null).await?;
                ordinary(&auth, group, user, id)?;
                if group["kind"] == "tenant" && index == 0 {
                    templates.push(template(group, ledger, &mut *session).await?);
                }
                Ok(())
            }
            .await;
            close(&mut *session, result).await?;
        }
    }
    let value = receipt::build(plan, &ledger.state, templates)?;
    receipt::validate(&value, plan, &ledger.state, &ledger.root)?;
    Ok(value)
}

async fn template(
    group: &Value,
    ledger: &Ledger,
    session: &mut dyn IdentitySession,
) -> Result<Value> {
    let path = ledger.root.join(format!(
        "template-{}-{}.xlsx",
        text(group, "slot")?,
        uuid::Uuid::new_v4()
    ));
    let receipt = session.template(&path).await?;
    Ok(json!({
        "tenant_slot": group["slot"], "path": path,
        "template_sha256": receipt["template_sha256"],
        "department_path": receipt["department_path"],
        "department_sha256": receipt["department_sha256"]
    }))
}

async fn verify_admin(group: &Value, ledger: &Ledger, context: &mut dyn Context) -> Result<()> {
    let role_id = text(&ledger.state["roles"], text(group, "slot")?)?;
    let mut admin = context.session(&group["admin"]).await?;
    let result = async {
        admin.login().await?;
        let role = request(
            &mut *admin,
            "get_system_roles_by_id",
            json!({"id": role_id}),
            Value::Null,
        )
        .await?;
        apply::role(&role, group, role_id)?;
        let tree = request(
            &mut *admin,
            "get_system_perms_tree",
            Value::Null,
            Value::Null,
        )
        .await?;
        let permissions = permission_ids(&tree, &group["permissions"])?;
        let assigned = request(
            &mut *admin,
            "get_system_roles_by_id_permissions",
            json!({"id": role_id}),
            Value::Null,
        )
        .await?;
        if !same_set(&assigned, &permissions) {
            return Err("角色实际权限与计划不同".into());
        }
        for user in array(&group["users"])? {
            let id = text(&ledger.state["users"], text(user, "user_slot")?)?;
            let detail = request(
                &mut *admin,
                "get_system_users_by_id",
                json!({"id": id}),
                Value::Null,
            )
            .await?;
            apply::administrative_user_detail(&detail, user, role_id, id)?;
        }
        Ok(())
    }
    .await;
    close(&mut *admin, result).await
}

pub(super) fn ordinary(value: &Value, group: &Value, user: &Value, id: &str) -> Result<()> {
    if value["user"]["id"] != id
        || value["user"]["tenant_id"] != group["tenant_id"]
        || value["user"]["username"] != user["username"]
        || value["password_change_required"] != false
        || value["is_super_admin"] != false
        || !same_set(&value["roles"], &json!([group["role_code"]]))
        || !same_set(&value["permissions"], &group["permissions"])
    {
        return Err("普通用户真实会话的身份、租户、首次改密状态或权限不匹配".into());
    }
    Ok(())
}
