use super::{
    evidence,
    plan::{array, text},
    run_lock, state,
};
use crate::{
    Result,
    recovery::clone::native::{digest, json as document},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) fn identities(plan: &Value, state: &Value) -> Result<Vec<Value>> {
    let mut identities = Vec::with_capacity(200);
    for group in array(&plan["groups"])? {
        for user in array(&group["users"])? {
            let slot = text(user, "user_slot")?;
            identities.push(json!({"tenant_slot":group["slot"],"user_slot":slot,"tenant_id":group["tenant_id"],"username":user["username"],"client_address":user["client_address"],"credential_slot":user["credential_slot"],"user_id":state::id(&state["users"][slot])?,"roles_sha256":digest::plan_hash(&json!([group["role_code"]]))?,"permissions_sha256":digest::plan_hash(&group["permissions"])?}));
        }
    }
    if identities.len() != 200 {
        return Err("身份验证投影必须为固定 200 用户".into());
    }
    Ok(identities)
}
pub(super) fn build(plan: &Value, state: &Value, templates: Vec<Value>) -> Result<Value> {
    let identities = identities(plan, state)?;
    let audience = identities
        .iter()
        .filter(|identity| identity["tenant_slot"] == "system")
        .take(10)
        .map(|identity| json!({"kind":"user","target_id":identity["user_id"]}))
        .collect::<Vec<_>>();
    let homepage = identities
        .iter()
        .filter(|identity| {
            identity["tenant_slot"] != "system"
                && identity["user_slot"]
                    .as_str()
                    .is_some_and(|slot| slot.ends_with("-001"))
        })
        .map(|identity| identity["user_slot"].clone())
        .collect::<Vec<_>>();
    Ok(
        json!({"format_version":1,"plan_sha256":plan["plan_sha256"],"scope_id":plan["environment"]["scope_id"],"status":"verified","identity_pools":pools(&identities)?,"identities":identities,"templates":templates,"message_audience":audience,"audience_sha256":digest::plan_hash(&json!(audience))?,"homepage_user_slots":homepage}),
    )
}
fn pools(identities: &[Value]) -> Result<Value> {
    let mut contract = serde_json::Map::new();
    let mut bindings = serde_json::Map::new();
    for (name, system) in [("system", true), ("tenants", false)] {
        let mut selected = identities
            .iter()
            .filter(|identity| (identity["tenant_slot"] == "system") == system)
            .collect::<Vec<_>>();
        if !system {
            selected.sort_by_key(|identity| {
                let slot = identity["user_slot"].as_str().unwrap_or("");
                (
                    slot.get(slot.len().saturating_sub(3)..).unwrap_or(""),
                    identity["tenant_slot"].as_str().unwrap_or(""),
                )
            });
        }
        if selected.len() != 100
            || selected.iter().any(|item| {
                item["roles_sha256"] != selected[0]["roles_sha256"]
                    || item["permissions_sha256"] != selected[0]["permissions_sha256"]
            })
        {
            return Err("身份池数量或授权摘要不一致".into());
        }
        let mut selections = serde_json::Map::new();
        for count in [10, 50, 100] {
            let mut distribution = BTreeMap::<&str, u64>::new();
            for item in selected.iter().take(count) {
                *distribution.entry(text(item, "tenant_slot")?).or_default() += 1;
            }
            selections.insert(count.to_string(), json!(distribution));
        }
        let slots=selected.iter().map(|item|json!({"tenant_slot":item["tenant_slot"],"user_slot":item["user_slot"],"client_address":item["client_address"]})).collect::<Vec<_>>();
        contract.insert(name.into(),json!({"roles_sha256":selected[0]["roles_sha256"],"permissions_sha256":selected[0]["permissions_sha256"],"client_address_model":"fixed-benchmark-per-user","slots":slots,"selections":selections}));
        bindings.insert(name.into(),json!(selected.iter().map(|item|json!({"tenant_id":item["tenant_id"],"username":item["username"],"credential_slot":item["credential_slot"]})).collect::<Vec<_>>()));
    }
    Ok(json!({"contract":contract,"bindings":bindings}))
}
pub(super) fn validate(value: &Value, plan: &Value, state: &Value, root: &Path) -> Result<()> {
    document::exact(
        value,
        &[
            "format_version",
            "plan_sha256",
            "scope_id",
            "status",
            "identities",
            "identity_pools",
            "templates",
            "message_audience",
            "audience_sha256",
            "homepage_user_slots",
        ],
        "身份验证收据",
    )?;
    let templates = array(&value["templates"])?;
    if *value != build(plan, state, templates.to_vec())? {
        return Err("身份验证收据投影与完整账本不同".into());
    }
    let groups = array(&plan["groups"])?;
    let tenants = groups
        .iter()
        .filter(|group| group["kind"] == "tenant")
        .collect::<Vec<_>>();
    if templates.len() != 10 || tenants.len() != 10 {
        return Err("身份收据缺少十租户模板".into());
    }
    let mut paths = BTreeSet::new();
    let mut departments = BTreeSet::new();
    for (item, group) in templates.iter().zip(tenants) {
        document::exact(
            item,
            &[
                "tenant_slot",
                "path",
                "template_sha256",
                "department_path",
                "department_sha256",
            ],
            "身份模板收据",
        )?;
        let path = Path::new(text(item, "path")?);
        let department = text(item, "department_path")?;
        let prefix = format!("template-{}-", text(group, "slot")?);
        let id = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(&prefix))
            .and_then(|name| name.strip_suffix(".xlsx"))
            .ok_or("身份模板名称不属于当前租户")?;
        let backend = Path::new(text(&plan["environment"], "backend_dir")?);
        crate::local_test_path::validate_local_test_path(
            path,
            backend,
            crate::local_test_path::LocalTestPathKind::ExistingFile,
        )?;
        if !run_lock::uuid(id)
            || path.parent() != Some(root)
            || item["tenant_slot"] != group["slot"]
            || item["department_sha256"] != evidence::hash(department)
            || evidence::hash(evidence::regular(path)?.ok_or("身份模板实物缺失")?)
                != text(item, "template_sha256")?
            || !paths.insert(path)
        {
            return Err("身份模板未绑定当前根目录、租户、部门或实物摘要".into());
        }
        departments.insert(department);
    }
    if departments.len() != 1 {
        return Err("十租户模板的完整部门路径不同".into());
    }
    Ok(())
}
