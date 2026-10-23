//! 部门目录、主体与完整前后像的唯一判定入口。
use std::{collections::BTreeSet, path::Path};

use serde_json::{Value, json};

use super::{document, evidence, history, plan, workbook};
use crate::Result;

pub(super) fn dispatch(backend: &Path, run: &Path, args: &Value) -> Result<Value> {
    document::exact(args, &["operation", "method", "input"], "部门模型请求")?;
    let value = super::common::read_bound(backend, &args["input"], "部门模型受管输入")?;
    let result = execute(backend, run, &args["method"], &value)?;
    super::common::bound_file(backend, &args["input"], "部门模型受管输入")?;
    Ok(result)
}

pub(super) fn execute(
    backend: &Path,
    run: &Path,
    method: &Value,
    args_value: &Value,
) -> Result<Value> {
    match document::text(method, "部门模型操作")? {
        "page-data" => {
            exact_args(args_value, &["response"])?;
            page_data(&args_value["response"])
        }
        "authorization" => {
            exact_args(args_value, &["target", "value", "write"])?;
            authorization(
                &args_value["target"],
                &args_value["value"],
                document::boolean(&args_value["write"], "部门写入权限")?,
            )
        }
        "created-response" => {
            exact_args(args_value, &["action", "target", "value"])?;
            created_response(
                &args_value["action"],
                &args_value["target"],
                &args_value["value"],
            )
        }
        "image" | "classify" => {
            exact_args(args_value, &["action", "observed", "target"])?;
            if method == "image" {
                image(
                    &args_value["action"],
                    &args_value["observed"],
                    &args_value["target"],
                )
            } else {
                Ok(json!(classify(
                    &args_value["action"],
                    &args_value["observed"],
                    &args_value["target"]
                )?))
            }
        }
        "template-image" => {
            exact_args(args_value, &["value", "target", "principal"])?;
            template_image(
                &args_value["value"],
                &args_value["target"],
                &args_value["principal"],
            )
        }
        "template-authorization" => {
            exact_args(args_value, &["target", "value", "principal"])?;
            template_authorization(
                &args_value["target"],
                &args_value["value"],
                &args_value["principal"],
            )
        }
        "intent" | "reconciliation" | "confirmation" | "action-history" => {
            history::dispatch(backend, run, method, args_value)
        }
        "identity-plan"
        | "targets"
        | "template-identities"
        | "read-prerequisite"
        | "require-prerequisite"
        | "validate-plan" => plan::dispatch(backend, run, method, args_value),
        "department-evidence" | "authorize-identity" => {
            evidence::dispatch(backend, run, method, args_value)
        }
        method => super::workflow::dispatch(backend, run, method, args_value),
    }
}

pub(super) fn exact_args(value: &Value, fields: &[&str]) -> Result<()> {
    document::exact(value, fields, "部门模型参数")?;
    Ok(())
}

pub(super) fn snowflake(value: &Value) -> Result<&str> {
    let text = document::text(value, "部门字符串 ID")?;
    if text.is_empty()
        || text.starts_with('0')
        || !text.bytes().all(|b| b.is_ascii_digit())
        || text.parse::<i64>().ok().filter(|n| *n > 0).is_none()
    {
        return Err("部门准备资源必须返回明确字符串 ID".into());
    }
    Ok(text)
}

pub(super) fn principal(value: &Value) -> Result<Value> {
    let object = document::object(value, "部门主体")?;
    for key in ["subject_id", "tenant_id", "username"] {
        if !object.contains_key(key) {
            return Err("部门主体字段缺失".into());
        }
    }
    Ok(
        json!({"subject_id":value["subject_id"],"tenant_id":value["tenant_id"],"username":value["username"]}),
    )
}

fn permission_list(value: &Value) -> Result<&[Value]> {
    document::exact(
        value,
        &[
            "subject_id",
            "tenant_id",
            "username",
            "is_super_admin",
            "permissions",
        ],
        "部门授权",
    )?;
    snowflake(&value["subject_id"])?;
    let permissions = document::array(&value["permissions"], "部门权限")?;
    if permissions.contains(&json!("*")) {
        return Err("部门主体不能使用通配权限".into());
    }
    Ok(permissions)
}

pub(super) fn authorization(target: &Value, value: &Value, write: bool) -> Result<Value> {
    let permissions = permission_list(value)?;
    if value["tenant_id"] != target["tenant_id"]
        || value["username"] != target["admin"]["username"]
        || !(value["is_super_admin"] == true
            || permissions.contains(&json!("system:dept:list"))
                && (!write || permissions.contains(&json!("system:dept:add"))))
    {
        return Err("部门动作租户管理员或权限不符".into());
    }
    principal(value)
}

pub(super) fn page_data(response: &Value) -> Result<Value> {
    if response["code"].as_i64() != Some(200) {
        return Err("部门观察缺少成功 API envelope".into());
    }
    let data = &response["data"];
    document::exact(
        data,
        &[
            "items",
            "page",
            "page_size",
            "total",
            "total_pages",
            "max_page_size",
        ],
        "部门分页",
    )?;
    let items = document::array(&data["items"], "部门完整列表")?;
    let total = data["total"].as_u64().ok_or("部门列表数量必须为非负整数")?;
    if data["page"].as_u64() != Some(1)
        || data["page_size"].as_u64() != Some(100)
        || data["max_page_size"].as_u64() != Some(100)
        || total > 100
        || total != items.len() as u64
        || data["total_pages"].as_u64() != Some(u64::from(total > 0))
    {
        return Err("部门列表不是完整第一页或数量不可证明".into());
    }
    Ok(data.clone())
}

fn department_record(value: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "id",
            "name",
            "parent_id",
            "ancestors",
            "sort",
            "status",
            "remark",
            "created_at",
        ],
        "部门记录",
    )?;
    snowflake(&value["id"])?;
    if !value["parent_id"].is_null() {
        snowflake(&value["parent_id"])?;
    }
    if document::text(&value["name"], "部门名称")?
        .trim()
        .is_empty()
        || !value["ancestors"].is_string()
        || value["sort"].as_i64().is_none()
        || !value["status"].is_string()
        || !(value["remark"].is_null() || value["remark"].is_string())
        || document::text(&value["created_at"], "部门创建时间")?.is_empty()
    {
        return Err("部门目录对象字段无效".into());
    }
    Ok(())
}

fn department(action: &Value, value: &Value) -> Result<Value> {
    department_record(value)?;
    for (key, expected) in document::object(&action["expected"], "部门预期字段")? {
        if value.get(key) != Some(expected) {
            return Err("部门对象不是计划中的唯一启用根部门".into());
        }
    }
    Ok(value.clone())
}

pub(super) fn created_response(action: &Value, target: &Value, value: &Value) -> Result<Value> {
    document::exact(value, &["response", "authorization"], "部门创建响应")?;
    if authorization(target, &value["authorization"], true)? != action["principal"]
        || value["response"]["code"].as_i64() != Some(200)
    {
        return Err("部门创建响应主体不同或缺少成功 API envelope".into());
    }
    department(action, &value["response"]["data"])
}

fn directory_image(data: &Value) -> Result<Value> {
    let mut rows = document::array(&data["items"], "部门列表")?.to_vec();
    let mut ids = BTreeSet::new();
    for row in &rows {
        department_record(row)?;
        if !ids.insert(snowflake(&row["id"])?) {
            return Err("部门完整目录包含重复 ID".into());
        }
    }
    rows.sort_by_key(|row| row["id"].as_str().and_then(|v| v.parse::<i64>().ok()));
    Ok(json!({"total":rows.len(),"departments":rows}))
}

fn template_image(value: &Value, target: &Value, expected: &Value) -> Result<Value> {
    document::exact(value, &["response", "authorization"], "部门模板")?;
    template_authorization(target, &value["authorization"], expected)?;
    workbook::template(&value["response"])
}

fn template_authorization(target: &Value, auth: &Value, expected: &Value) -> Result<Value> {
    let permissions = permission_list(auth)?;
    if principal(auth)? != *expected
        || auth["tenant_id"] != target["tenant_id"]
        || auth["is_super_admin"] != false
        || !permissions.contains(&json!("system:user-import:add"))
    {
        return Err("部门模板固定用户或导入权限不符".into());
    }
    principal(auth)
}

pub(super) fn image(action: &Value, observed: &Value, target: &Value) -> Result<Value> {
    document::exact(observed, &["list", "detail", "template"], "部门观察")?;
    let listed = &observed["list"];
    document::exact(listed, &["response", "authorization"], "部门列表观察")?;
    let who = authorization(target, &listed["authorization"], false)?;
    if !action["principal"].is_null() && who != action["principal"] {
        return Err("部门目录主体不同于固定计划主体".into());
    }
    let directory = directory_image(&page_data(&listed["response"])?)?;
    let matches = document::array(&directory["departments"], "部门目录")?
        .iter()
        .filter(|row| row["name"] == action["body"]["name"])
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err("部门目录含多个固定名称对象".into());
    }
    let item = matches
        .first()
        .map(|row| department(action, row))
        .transpose()?
        .unwrap_or(Value::Null);
    let detail = &observed["detail"];
    if item.is_null() {
        if !detail.is_null() {
            return Err("固定部门不存在时不能带详情".into());
        }
    } else {
        document::exact(detail, &["response", "authorization"], "部门详情")?;
        if authorization(target, &detail["authorization"], false)? != who
            || detail["response"]["code"].as_i64() != Some(200)
            || detail["response"]["data"] != item
        {
            return Err("部门详情与完整列表对象或主体不同".into());
        }
    }
    Ok(
        json!({"directory":directory,"department":item,"template":template_image(&observed["template"], target, &action["template_principal"] )?,
        "principal":who,"template_principal":action["template_principal"]}),
    )
}

pub(super) fn classify(action: &Value, observed: &Value, target: &Value) -> Result<&'static str> {
    let current = image(action, observed, target)?;
    let name = &action["body"]["name"];
    let paths = document::array(&action["before_template_paths"], "部门模板前像")?;
    if current["department"].is_null()
        && current["directory"] == action["before"]
        && current["template"]["paths"] == action["before_template_paths"]
        && !paths.contains(name)
    {
        return Ok("before");
    }
    let mut rows = document::array(&action["before"]["departments"], "部门目录前像")?.to_vec();
    rows.push(current["department"].clone());
    rows.sort_by_key(|row| row["id"].as_str().and_then(|v| v.parse::<i64>().ok()));
    let mut after_paths = paths.to_vec();
    after_paths.push(name.clone());
    after_paths.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    let total = action["before"]["total"]
        .as_u64()
        .and_then(|v| v.checked_add(1))
        .ok_or("部门前像数量无效")?;
    if !current["department"].is_null()
        && current["directory"] == json!({"total":total,"departments":rows})
        && current["template"]["paths"] == json!(after_paths)
    {
        return Ok("after");
    }
    Ok("mismatch")
}
