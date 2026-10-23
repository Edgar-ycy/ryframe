//! 按确定性写入顺序核对身份账本；未知写入不能被视为已确认前缀。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde_json::Value;

use super::plan::{array, text};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::json,
};

pub(crate) fn verify_input(plan: &Value, directory: &Path, backend: &Path) -> Result<()> {
    let filename = directory.join("ledger.json");
    validate_local_test_path(&filename, backend, LocalTestPathKind::ExistingFile)?;
    validate(plan, &json::read(&filename)?, true)
}

pub(crate) fn expected(plan: &Value) -> Result<Vec<(String, String)>> {
    let mut operations = Vec::with_capacity(622);
    for group in array(&plan["groups"])? {
        let slot = text(group, "slot")?;
        for operation in ["post_system_roles", "put_system_roles_by_id_permissions"] {
            operations.push((operation.into(), slot.into()));
        }
        for user in array(&group["users"])? {
            for operation in [
                "post_system_users",
                "post_system_users_by_id_password_reset_requests",
                "post_auth_password_reset_complete",
            ] {
                operations.push((operation.into(), text(user, "user_slot")?.into()));
            }
        }
    }
    Ok(operations)
}

pub(crate) fn validate(plan: &Value, state: &Value, complete: bool) -> Result<()> {
    let status = text(state, "status")?;
    let mut fields = vec![
        "format_version",
        "plan_sha256",
        "status",
        "entries",
        "roles",
        "users",
    ];
    if matches!(status, "verifying" | "verified") {
        fields.push("publication_nonce");
    }
    if status == "verified" {
        fields.push("verified_receipt");
    }
    json::exact(state, &fields, "身份账本")?;
    if state["format_version"] != 1
        || state["plan_sha256"] != plan["plan_sha256"]
        || !matches!(status, "applying" | "prepared" | "verifying" | "verified")
        || (complete && status == "applying")
    {
        return Err("身份账本不属于当前可恢复计划或仍有未知写入；禁止重放".into());
    }
    let expected = expected(plan)?;
    let entries = array(&state["entries"])?;
    if entries.len() > expected.len()
        || ((complete || status != "applying") && entries.len() != expected.len())
    {
        return Err("身份账本不是完整写入序列或合法前缀".into());
    }
    let (roles, users) = entries_projection(entries, &expected)?;
    let actual_roles = json::object(&state["roles"], "已确认角色")?;
    let actual_users = json::object(&state["users"], "已确认用户")?;
    let record = |expected: &BTreeMap<String, String>, actual: &serde_json::Map<String, Value>| {
        expected.len() == actual.len()
            && expected
                .iter()
                .all(|(slot, id)| actual.get(slot).and_then(Value::as_str) == Some(id))
    };
    if !record(&roles, actual_roles)
        || !record(&users, actual_users)
        || roles
            .values()
            .chain(users.values())
            .collect::<BTreeSet<_>>()
            .len()
            != roles.len() + users.len()
    {
        return Err("身份账本资源映射与已确认前缀不一致或资源 ID 重复".into());
    }
    publication(state, status)
}

type Projection = (BTreeMap<String, String>, BTreeMap<String, String>);

fn entries_projection(entries: &[Value], expected: &[(String, String)]) -> Result<Projection> {
    let mut roles = BTreeMap::new();
    let mut users = BTreeMap::new();
    let mut requests = BTreeSet::new();
    for (entry, (operation, slot)) in entries.iter().zip(expected) {
        let reset = operation == "post_system_users_by_id_password_reset_requests";
        let fields = if reset {
            vec!["operation", "slot", "phase", "id", "request_id"]
        } else {
            vec!["operation", "slot", "phase", "id"]
        };
        json::exact(entry, &fields, "身份写入记录")?;
        if entry["operation"] != operation.as_str()
            || entry["slot"] != slot.as_str()
            || entry["phase"] != "confirmed"
        {
            return Err("身份账本不是确定性已确认前缀；需要核对未知结果".into());
        }
        let id = id(&entry["id"])?;
        if reset && !requests.insert(id_value(&entry["request_id"])?.to_owned()) {
            return Err("身份账本密码请求 ID 重复".into());
        }
        match operation.as_str() {
            "post_system_roles" => {
                roles.insert(slot.clone(), id.into());
            }
            "post_system_users" => {
                users.insert(slot.clone(), id.into());
            }
            "put_system_roles_by_id_permissions"
                if roles.get(slot).map(String::as_str) == Some(id) => {}
            "post_system_users_by_id_password_reset_requests"
            | "post_auth_password_reset_complete"
                if users.get(slot).map(String::as_str) == Some(id) => {}
            _ => return Err("身份写入未绑定先前确认的角色或用户".into()),
        }
    }
    Ok((roles, users))
}

pub(crate) fn id(value: &Value) -> Result<&str> {
    id_value(value)
}

fn id_value(value: &Value) -> Result<&str> {
    let text = value.as_str().ok_or("身份资源 ID 必须为字符串")?;
    if text.starts_with('0')
        || text.is_empty()
        || !text.bytes().all(|byte| byte.is_ascii_digit())
        || !text.parse::<i64>().is_ok_and(|id| id > 0)
    {
        return Err("身份资源 ID 超出正整数范围或写法不规范".into());
    }
    Ok(text)
}

fn publication(state: &Value, status: &str) -> Result<()> {
    if matches!(status, "verifying" | "verified") {
        let nonce = text(state, "publication_nonce")?;
        if !uuid::Uuid::parse_str(nonce).is_ok_and(|id| {
            id.get_version_num() == 4
                && id.get_variant() == uuid::Variant::RFC4122
                && id.to_string() == nonce
        }) {
            return Err("身份账本没有绑定唯一发布会话".into());
        }
    }
    if status == "verified" {
        let filename = text(state, "verified_receipt")?;
        let id = filename
            .strip_prefix("verified-")
            .and_then(|name| name.strip_suffix(".json"))
            .ok_or("身份收据文件名无效")?;
        if id.len() != 36
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
        {
            return Err("verified 身份账本未绑定明确收据".into());
        }
    }
    Ok(())
}
