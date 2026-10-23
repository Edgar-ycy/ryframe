//! 十租户部门计划及身份前提的只读闭环；复用既有完整身份投影。
use std::{collections::BTreeSet, path::Path};

use regex::Regex;
use serde_json::{Value, json};

use super::{common, digest, document, model};
use crate::Result;

pub(super) fn dispatch(backend: &Path, run: &Path, method: &Value, args: &Value) -> Result<Value> {
    let method = document::text(method, "部门计划操作")?;
    let fields: &[&str] = match method {
        "targets" => &["plan"],
        "read-prerequisite" => &[],
        "require-prerequisite" => &["request", "value"],
        _ => &["request"],
    };
    model::exact_args(args, fields)?;
    match method {
        "identity-plan" => identity_plan(backend, &args["request"]),
        "targets" => Ok(json!(targets(&args["plan"])?)),
        "template-identities" => template_identities(backend, run, &args["request"]),
        "read-prerequisite" => read_prerequisite(backend, run),
        "require-prerequisite" => {
            let expected = &args["value"]["identity"];
            if identity_projection(
                backend,
                run,
                &args["request"],
                expected["state"] == "absent",
            )? != *expected
            {
                return Err("部门阶段期间身份账本前提发生变化".into());
            }
            Ok(args["value"].clone())
        }
        "validate-plan" => validate_plan(backend, run, &args["request"]),
        _ => Err("未知部门计划操作".into()),
    }
}

pub(super) fn identity_plan(backend: &Path, request: &Value) -> Result<Value> {
    let mut plan = common::read_bound(backend, &request["identity_plan"], "部门身份计划")?;
    document::exact(
        &plan,
        &[
            "format_version",
            "kind",
            "configuration",
            "environment",
            "groups",
            "plan_sha256",
        ],
        "部门身份计划",
    )?;
    let hash = plan
        .as_object_mut()
        .ok_or("部门身份计划不是对象")?
        .remove("plan_sha256")
        .ok_or("部门身份计划摘要缺失")?;
    if plan["format_version"].as_u64() != Some(1)
        || plan["kind"] != "devex-identities"
        || hash != digest::plan_hash(&plan)?
    {
        return Err("部门阶段引用的身份计划摘要无效".into());
    }
    plan["plan_sha256"] = hash;
    common::bound_file(backend, &request["identity_plan"], "部门身份计划")?;
    Ok(plan)
}

pub(super) fn targets(plan: &Value) -> Result<Vec<Value>> {
    let groups = document::array(&plan["groups"], "部门身份组")?;
    if groups.len() != 11 || groups[0]["slot"] != "system" {
        return Err("部门阶段只允许身份计划中的十个普通租户".into());
    }
    let name = format!(
        "dv-{}-import",
        document::text(&plan["environment"]["plan_id"], "部门计划 ID")?
    );
    if name.chars().count() > 50 {
        return Err("固定部门名称超过当前契约".into());
    }
    let mut result = Vec::new();
    for (index, group) in groups[1..].iter().enumerate() {
        if group["slot"] != format!("tenant-{:02}", index + 1) || group["kind"] != "tenant" {
            return Err("部门阶段只允许身份计划中的十个普通租户".into());
        }
        for field in ["tenant_id", "admin"] {
            if group.get(field).is_none() {
                return Err("部门目标字段缺失".into());
            }
        }
        result.push(json!({"slot":group["slot"],"tenant_id":group["tenant_id"],"admin":group["admin"],
            "body":{"name":name,"parent_id":null,"sort":0},
            "expected":{"name":name,"parent_id":null,"ancestors":"0","sort":0,"status":"1","remark":null}}));
    }
    Ok(result)
}

pub(super) fn identity_projection(
    backend: &Path,
    run: &Path,
    request: &Value,
    allow_absent: bool,
) -> Result<Value> {
    super::projection(
        backend,
        run,
        &json!({"operation":"identity-projection",
        "request":{"identity_plan":request["identity_plan"],"identity_state":request["identity_state"]},"allow_absent":allow_absent}),
    )
}

pub(super) fn template_identities(backend: &Path, run: &Path, request: &Value) -> Result<Value> {
    let plan = identity_plan(backend, request)?;
    let projected = identity_projection(backend, run, request, false)?;
    if !matches!(projected["state"].as_str(), Some("prepared" | "verified")) {
        return Err("部门模板需要已确认的固定身份账本".into());
    }
    let ledger = common::read_bound(backend, &projected["ledger"], "部门模板身份账本")?;
    let groups = document::array(&plan["groups"], "部门模板身份组")?;
    let mut identities = Vec::new();
    let mut subjects = BTreeSet::new();
    for (index, group) in groups.iter().skip(1).enumerate() {
        let permissions = document::array(&group["permissions"], "部门模板权限")?;
        let users = document::array(&group["users"], "部门模板用户")?;
        if group["slot"] != format!("tenant-{:02}", index + 1)
            || group["kind"] != "tenant"
            || permissions.contains(&json!("*"))
            || !permissions.contains(&json!("system:user-import:add"))
        {
            return Err("部门模板用户不属于具备导入权限的固定租户身份组".into());
        }
        let user = users.first().ok_or("部门模板缺少固定用户")?;
        document::exact(
            user,
            &["user_slot", "username", "credential_slot", "client_address"],
            "部门模板用户",
        )?;
        let slot = document::text(&user["user_slot"], "部门模板用户槽")?;
        let id = model::snowflake(&ledger["users"][slot])?;
        subjects.insert(id.to_owned());
        identities.push(
            json!({"subject_id":id,"tenant_id":group["tenant_id"],"username":user["username"],
            "credential_slot":user["credential_slot"],"client_address":user["client_address"]}),
        );
    }
    if identities.len() != 10 || subjects.len() != 10 {
        return Err("部门模板固定主体数量不足或重复".into());
    }
    common::bound_file(backend, &projected["ledger"], "部门模板身份账本")?;
    Ok(json!(identities))
}

pub(super) fn read_prerequisite(backend: &Path, run: &Path) -> Result<Value> {
    let path = run.join("seed-runtime/departments/identity-prerequisite.json");
    let bound = common::binding(&path)?;
    let value = common::read_bound(backend, &bound, "部门身份前提")?;
    document::exact(
        &value,
        &["format_version", "kind", "registration", "identity"],
        "部门身份前提",
    )?;
    document::exact(
        &value["identity"],
        &[
            "state",
            "identity_state",
            "plan_sha256",
            "ledger",
            "projection_sha256",
            "entries",
            "roles",
            "users",
        ],
        "部门身份前提投影",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-seed-identity-prerequisite"
        || value["registration"] != common::binding(&run.join("seed-runtime.json"))?
        || !matches!(
            value["identity"]["state"].as_str(),
            Some("absent" | "prepared")
        )
    {
        return Err("部门计划的身份前提文件无效".into());
    }
    common::bound_file(backend, &bound, "部门身份前提")?;
    Ok(value)
}

pub(super) fn validate_plan(backend: &Path, run: &Path, request: &Value) -> Result<Value> {
    let path = run.join("seed-runtime/department-plan.json");
    let bound = common::binding(&path)?;
    let mut plan = common::read_bound(backend, &bound, "部门计划")?;
    document::exact(
        &plan,
        &[
            "format_version",
            "kind",
            "registration",
            "identity_plan",
            "prerequisite",
            "actions",
            "plan_sha256",
        ],
        "部门计划",
    )?;
    let source = identity_plan(backend, request)?;
    let expected_targets = targets(&source)?;
    let templates = template_identities(backend, run, request)?;
    let hash = plan
        .as_object_mut()
        .ok_or("部门计划不是对象")?
        .remove("plan_sha256")
        .ok_or("部门计划摘要缺失")?;
    if plan["format_version"].as_u64() != Some(1)
        || plan["kind"] != "devex-seed-departments"
        || plan["registration"] != common::binding(&run.join("seed-runtime.json"))?
        || plan["identity_plan"] != request["identity_plan"]
        || plan["prerequisite"]
            != common::binding(&run.join("seed-runtime/departments/identity-prerequisite.json"))?
        || hash != digest::plan_hash(&plan)?
    {
        return Err("部门计划不属于当前 seed 登记或原身份计划".into());
    }
    plan["plan_sha256"] = hash;
    let actions = document::array(&plan["actions"], "部门动作")?;
    if actions.len() != 10 {
        return Err("部门动作必须覆盖十租户".into());
    }
    for (index, (action, expected)) in actions.iter().zip(&expected_targets).enumerate() {
        validate_action(backend, run, action, expected, &templates[index])?;
    }
    read_prerequisite(backend, run)?;
    common::bound_file(backend, &bound, "部门计划")?;
    Ok(plan)
}

fn validate_action(
    backend: &Path,
    run: &Path,
    action: &Value,
    expected: &Value,
    template: &Value,
) -> Result<()> {
    document::exact(
        action,
        &[
            "target",
            "body",
            "expected",
            "before",
            "before_evidence",
            "before_template_paths",
            "principal",
            "template_principal",
        ],
        "部门计划动作",
    )?;
    if action["target"] != json!({"slot":expected["slot"],"tenant_id":expected["tenant_id"]})
        || action["body"] != expected["body"]
        || action["expected"] != expected["expected"]
        || action["template_principal"] != model::principal(template)?
    {
        return Err("部门动作不是十租户固定根部门".into());
    }
    let evidence = common::bound_file(backend, &action["before_evidence"], "部门计划前像")?;
    if evidence.file_name().and_then(|v| v.to_str())
        != Some(
            format!(
                "{}-before.json",
                document::text(&expected["slot"], "部门槽")?
            )
            .as_str(),
        )
        || evidence.parent().and_then(Path::parent) != Some(run.join("seed-runtime").as_path())
        || !evidence
            .parent()
            .and_then(Path::file_name)
            .and_then(|v| v.to_str())
            .is_some_and(|name| {
                Regex::new(r"^attempt-[0-9]{4,}$").is_ok_and(|re| re.is_match(name))
            })
    {
        return Err("部门计划前像证据不属于当前 seed 阶段".into());
    }
    let observed = model::image(
        action,
        &common::read_bound(backend, &action["before_evidence"], "部门计划前像")?,
        expected,
    )?;
    if observed["principal"] != action["principal"]
        || observed["template_principal"] != action["template_principal"]
        || observed["directory"] != action["before"]
        || !observed["department"].is_null()
        || observed["template"]["paths"] != action["before_template_paths"]
        || document::array(&observed["template"]["paths"], "部门模板路径")?
            .contains(&expected["body"]["name"])
    {
        return Err("部门计划未绑定完整目录且固定名称不存在的前像".into());
    }
    Ok(())
}
