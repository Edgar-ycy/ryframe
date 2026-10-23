//! 身份计划的离线事实模型；不读取凭据环境，也不探测服务。

use std::{collections::BTreeSet, path::Path};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{digest, json as document},
};

#[path = "plan/validation.rs"]
mod validation;

pub(crate) fn create(environment: &Value, backend: &Path) -> Result<Value> {
    validation::environment(environment)?;
    if Path::new(text(environment, "backend_dir")?) != backend {
        return Err("必须在身份清单绑定的后端目录运行".into());
    }
    validate_sources(environment, backend)?;
    let mut groups = Vec::with_capacity(11);
    groups.push(group(
        environment,
        "system",
        "system",
        &environment["system_admin"],
        100,
        0,
    )?);
    for (index, tenant) in array(&environment["tenants"])?.iter().enumerate() {
        groups.push(group(
            environment,
            text(tenant, "slot")?,
            text(tenant, "tenant_id")?,
            &tenant["admin"],
            10,
            index + 1,
        )?);
    }
    let administrators = groups
        .iter()
        .map(|group| text(&group["admin"], "client_address"))
        .collect::<Result<BTreeSet<_>>>()?;
    for group in &groups {
        for user in array(&group["users"])? {
            if administrators.contains(text(user, "client_address")?) {
                return Err("普通用户固定客户地址不能与准备管理员重叠".into());
            }
        }
    }
    let mut plan = json!({"format_version":1,"kind":"devex-identities","environment":environment,"groups":groups});
    plan["plan_sha256"] = json!(digest::plan_hash(&plan)?);
    validate(&plan)?;
    Ok(plan)
}

pub(crate) fn write(environment: &Path, output: &Path, backend: &Path) -> Result<Value> {
    validate_local_test_path(environment, backend, LocalTestPathKind::ExistingFile)?;
    validate_local_test_path(output, backend, LocalTestPathKind::OutputFile)?;
    let before = digest::file(environment)?;
    let plan = create(&document::read(environment)?, backend)?;
    if digest::file(environment)? != before {
        return Err("身份计划生成期间环境清单变化；未发布计划，请核对来源".into());
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    validate_local_test_path(output, backend, LocalTestPathKind::OutputFile)?;
    document::write_new(output, &plan)?;
    Ok(json!({"status":"planned","users":200,"plan_sha256":plan["plan_sha256"]}))
}

pub(crate) fn validate(plan: &Value) -> Result<()> {
    document::exact(
        plan,
        &[
            "format_version",
            "kind",
            "environment",
            "groups",
            "plan_sha256",
        ],
        "身份计划",
    )?;
    validation::environment(&plan["environment"])?;
    let mut payload = plan.clone();
    payload
        .as_object_mut()
        .ok_or("身份计划必须为对象")?
        .remove("plan_sha256");
    if plan["format_version"] != 1
        || plan["kind"] != "devex-identities"
        || plan["plan_sha256"].as_str() != Some(digest::plan_hash(&payload)?.as_str())
    {
        return Err("身份计划摘要、类型或版本无效".into());
    }
    Ok(())
}

fn validate_sources(environment: &Value, backend: &Path) -> Result<()> {
    let declaration = &environment["environment_document"];
    let path = Path::new(text(declaration, "path")?);
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)?;
    if std::fs::read_to_string(path)?.trim().is_empty()
        || digest::file(path)?.sha256 != text(declaration, "sha256")?
    {
        return Err("环境说明为空或 SHA-256 与身份清单不符".into());
    }
    let runtime = &environment["runtime"];
    let directory = Path::new(text(runtime, "directory")?);
    validate_local_test_path(directory, backend, LocalTestPathKind::ExistingDirectory)?;
    for (filename, key) in [
        ("runtime.json", "receipt_sha256"),
        ("api.json", "api_process_sha256"),
    ] {
        let path = directory.join(filename);
        validate_local_test_path(&path, backend, LocalTestPathKind::ExistingFile)?;
        if digest::file(&path)?.sha256 != text(runtime, key)? {
            return Err("运行收据 SHA-256 与身份清单不符".into());
        }
    }
    let budget = Path::new(text(
        &environment["pacing"]["bindings"],
        "login_budget_state",
    )?);
    validate_local_test_path(budget, backend, LocalTestPathKind::OutputFile)
        .or_else(|_| validate_local_test_path(budget, backend, LocalTestPathKind::ExistingFile))?;
    Ok(())
}

fn group(
    environment: &Value,
    slot: &str,
    tenant: &str,
    admin: &Value,
    count: usize,
    index: usize,
) -> Result<Value> {
    let kind = if index == 0 { "system" } else { "tenant" };
    let plan_id = text(environment, "plan_id")?;
    let role = format!("dv-{plan_id}-{kind}");
    let users = (1..=count).map(|number| {
        json!({"user_slot":format!("{slot}-user-{number:03}"),"username":format!("dv-{plan_id}-{number:03}"),
            "password_env":environment["passwords"][format!("{kind}_env")],"client_address":format!("198.18.{}.{number}",40+index)})
    }).collect::<Vec<_>>();
    if role.len() > 50
        || users
            .iter()
            .any(|user| user["username"].as_str().is_none_or(|name| name.len() > 50))
    {
        return Err("准备名称超出当前接口的 50 字节限制".into());
    }
    Ok(
        json!({"slot":slot,"tenant_id":tenant,"admin":admin,"count":count,"kind":kind,
        "role_code":role,"permissions":permissions(kind),"users":users}),
    )
}

fn permissions(kind: &str) -> Vec<&'static str> {
    let mut values = if kind == "system" {
        vec![
            "monitor:schedule:add",
            "monitor:schedule:run",
            "system:message:publish",
            "system:post:export",
        ]
    } else {
        vec![
            "system:post:add",
            "system:post:edit",
            "system:post:export",
            "system:post:list",
            "system:post:remove",
            "system:user-import:add",
            "system:user-import:list",
        ]
    };
    values.sort_unstable();
    values
}

pub(super) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("身份字段 {key} 必须为非空字符串").into())
}

pub(super) fn array(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "身份字段必须为数组".into())
}
