//! 配额动作的纯计划、授权与完整前后像，不承担资源访问或写入。
use crate::{Result, recovery::clone::native::json as document};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const DETAIL_FIELDS: &[&str] = &[
    "name",
    "domain",
    "expire_at",
    "max_users",
    "max_roles",
    "max_storage_mb",
    "max_requests_per_min",
    "tenant_id",
    "status",
    "usage",
    "expiration_status",
    "capacity_status",
];

pub(crate) fn targets(identity: &Value) -> Result<Vec<Value>> {
    document::object(identity, "身份计划")?;
    let environment = document::object(&identity["environment"], "身份环境")?;
    let quota = document::exact(
        environment.get("quota").ok_or("身份环境缺少配额")?,
        &[
            "system_max_users",
            "tenant_max_users",
            "import_headroom_per_tenant",
        ],
        "身份配额",
    )?;
    for value in quota.values() {
        positive_quota(value)?;
    }
    let tenants = document::array(
        environment.get("tenants").ok_or("身份环境缺少租户")?,
        "身份租户",
    )?;
    let groups = document::array(&identity["groups"], "身份组")?;
    if tenants.len() != 10 || groups.len() != 11 {
        return Err("配额只允许系统及计划明确的十个租户".into());
    }
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let (slot, tenant, kind, count) = if index == 0 {
            ("system".to_owned(), "system", "system", 100_u64)
        } else {
            let entry = &tenants[index - 1];
            document::object(entry, "身份租户")?;
            (
                format!("tenant-{index:02}"),
                document::text(&entry["tenant_id"], "身份租户编号")?,
                "tenant",
                10,
            )
        };
        if tenant.is_empty()
            || tenant.len() > 64
            || !tenant
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || !seen.insert(tenant)
        {
            return Err("配额只允许系统及计划明确的十个租户".into());
        }
        document::object(group, "身份组")?;
        if group["slot"] != slot
            || group["tenant_id"] != tenant
            || group["kind"] != kind
            || group["count"].as_u64() != Some(count)
            || document::array(&group["users"], "身份用户")?.len() as u64 != count
        {
            return Err("身份组必须保持固定 100 加 10×10 用户".into());
        }
        result.push(json!({"slot":slot,"tenant_id":tenant,"new_users":count,
            "import_headroom":if kind=="tenant" { quota["import_headroom_per_tenant"].clone() } else { json!(0) },
            "max_users":quota[format!("{kind}_max_users").as_str()]}));
    }
    Ok(result)
}

pub(crate) fn data(response: &Value) -> Result<&Value> {
    if !response.is_object()
        || response["code"].as_u64() != Some(200)
        || !response["data"].is_object()
    {
        return Err("配额证据缺少成功的原始 API envelope".into());
    }
    Ok(&response["data"])
}

pub(crate) fn authorization(post: &Value, auth: &Value) -> Result<()> {
    document::object(post, "配额登记")?;
    let admin = document::object(&post["source_admin"], "登记管理员")?;
    document::object(auth, "管理员授权")?;
    for key in ["subject_id", "tenant_id", "username"] {
        let expected = document::text(
            admin.get(key).ok_or("登记管理员身份不完整")?,
            "登记管理员身份",
        )?;
        if auth[key].as_str() != Some(expected) {
            return Err("配额动作必须保持登记的系统管理员及全部必需权限".into());
        }
    }
    let permissions = document::array(&auth["permissions"], "管理员权限")?;
    let contains = |permission: &str| permissions.iter().any(|v| v.as_str() == Some(permission));
    if auth["tenant_id"] != "system"
        || (auth["is_super_admin"] != true
            && !contains("*")
            && !["tenant:list", "tenant:edit", "tenant:usage:list"]
                .iter()
                .all(|p| contains(p)))
    {
        return Err("配额动作必须保持登记的系统管理员及全部必需权限".into());
    }
    Ok(())
}

fn image(tenant_id: &Value, observed: &Value) -> Result<Value> {
    let tenant_id = document::text(tenant_id, "配额租户编号")?;
    document::exact(observed, &["tenant", "usage"], "租户观察")?;
    let detail = document::exact(data(&observed["tenant"])?, DETAIL_FIELDS, "租户详情")?;
    let usage = data(&observed["usage"])?;
    if detail["tenant_id"] != tenant_id
        || usage["tenant_id"] != tenant_id
        || detail["status"] != "enabled"
        || detail["name"].as_str().is_none_or(str::is_empty)
        || ["domain", "expire_at"]
            .iter()
            .any(|k| !detail[*k].is_null() && !detail[*k].is_string())
        || !matches!(
            detail["expiration_status"].as_str(),
            Some("never" | "active" | "expiring")
        )
    {
        return Err("租户身份、启用状态或正配额不完整".into());
    }
    for key in [
        "max_users",
        "max_roles",
        "max_storage_mb",
        "max_requests_per_min",
    ] {
        unsigned(&detail[key])?;
    }
    if unsigned(&detail["max_users"])? == 0 {
        return Err("租户身份、启用状态或正配额不完整".into());
    }
    let users = usage_count(detail, usage, "users", "max_users")?;
    let roles = usage_count(detail, usage, "roles", "max_roles")?;
    let mut tenant = detail.clone();
    for key in ["usage", "expiration_status", "capacity_status"] {
        tenant.remove(key);
    }
    Ok(json!({"tenant":tenant,"users":users,"roles":roles}))
}

fn usage_count(
    detail: &serde_json::Map<String, Value>,
    usage: &Value,
    resource: &str,
    maximum: &str,
) -> Result<u64> {
    let item = document::object(&usage[resource], "租户用量")?;
    let embedded = document::object(&detail["usage"][resource], "内嵌租户用量")?;
    let used = unsigned(item.get("used").ok_or("租户用量缺少计数")?)?;
    let maximum = unsigned(&detail[maximum])?;
    let limit = if maximum > 0 {
        json!(maximum)
    } else {
        Value::Null
    };
    if item.get("limit") != Some(&limit)
        || embedded.get("used") != item.get("used")
        || embedded.get("limit") != item.get("limit")
    {
        return Err("租户用量与当前配额不一致".into());
    }
    Ok(used)
}

pub(crate) fn action(target: &Value, observed: &Value) -> Result<Value> {
    document::object(target, "配额目标")?;
    let before = image(&target["tenant_id"], observed)?;
    let maximum = positive_quota(&target["max_users"])?;
    let required = unsigned(&before["users"])?
        .checked_add(unsigned(&target["new_users"])?)
        .and_then(|n| n.checked_add(target["import_headroom"].as_u64()?))
        .ok_or("配额计数类型无效或加法溢出")?;
    let max_roles = unsigned(&before["tenant"]["max_roles"])?;
    let role_capacity = max_roles == 0
        || unsigned(&before["roles"])?
            .checked_add(1)
            .is_some_and(|n| n <= max_roles);
    if maximum < unsigned(&before["tenant"]["max_users"])? || maximum < required || !role_capacity {
        return Err("配额不能缩减，必须覆盖实际用量、普通身份、导入余量及新增角色".into());
    }
    let mut after = before.clone();
    after["tenant"]["max_users"] = target["max_users"].clone();
    Ok(json!({"target":target.clone(),"before":before,"after":after,"observed":observed.clone()}))
}

pub(crate) fn classification(item: &Value, observed: &Value) -> Result<&'static str> {
    document::object(item, "配额动作")?;
    let target = document::object(&item["target"], "配额目标")?;
    let current = image(target.get("tenant_id").ok_or("配额目标缺少租户")?, observed)?;
    let after = item.get("after").ok_or("配额动作缺少后像")?;
    let before = item.get("before").ok_or("配额动作缺少前像")?;
    Ok(if current == *after {
        "after"
    } else if current == *before {
        "before"
    } else {
        "mismatch"
    })
}

fn unsigned(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .ok_or_else(|| "配额计数必须为非负整数".into())
}
fn positive_quota(value: &Value) -> Result<u64> {
    unsigned(value).and_then(|n| {
        if (1..=i32::MAX as u64).contains(&n) {
            Ok(n)
        } else {
            Err("配额和导入余量必须由身份环境明确登记正整数".into())
        }
    })
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/capacity/model.rs"]
mod tests;
