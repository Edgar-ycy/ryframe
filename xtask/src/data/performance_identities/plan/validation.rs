use std::{collections::BTreeSet, net::Ipv4Addr, path::Path};

use serde_json::Value;

use super::{array, text};
use crate::{Result, recovery::clone::native::json};

pub(super) fn environment(value: &Value) -> Result<()> {
    json::exact(
        value,
        &[
            "format_version",
            "plan_id",
            "scope_id",
            "backend_dir",
            "frontend_dir",
            "api_url",
            "frontend_url",
            "environment_document",
            "runtime",
            "database",
            "system_admin",
            "tenants",
            "passwords",
            "quota",
            "pacing",
        ],
        "身份环境清单",
    )?;
    if value["format_version"] != 1
        || !identifier(text(value, "plan_id")?, 32)
        || !identifier(text(value, "scope_id")?, 64)
    {
        return Err("身份计划版本或 scope 无效".into());
    }
    for key in ["backend_dir", "frontend_dir"] {
        absolute(text(value, key)?)?;
    }
    for key in ["api_url", "frontend_url"] {
        endpoint(text(value, key)?)?;
    }
    json::exact(
        &value["environment_document"],
        &["path", "sha256"],
        "环境说明",
    )?;
    json::exact(
        &value["runtime"],
        &["directory", "receipt_sha256", "api_process_sha256"],
        "运行来源",
    )?;
    for hash in [
        &value["environment_document"]["sha256"],
        &value["runtime"]["receipt_sha256"],
        &value["runtime"]["api_process_sha256"],
    ] {
        if !sha256(hash) {
            return Err("身份来源必须绑定真实 SHA-256".into());
        }
    }
    database(&value["database"])?;
    credential(&value["system_admin"], "system")?;
    tenants(value)?;
    json::exact(
        &value["passwords"],
        &["system_env", "tenant_env"],
        "普通用户密码引用",
    )?;
    for key in ["system_env", "tenant_env"] {
        secret_name(text(&value["passwords"], key)?)?;
    }
    json::exact(
        &value["quota"],
        &[
            "system_max_users",
            "tenant_max_users",
            "import_headroom_per_tenant",
        ],
        "预登记配额",
    )?;
    let quota = &value["quota"];
    let number = |key| {
        quota[key]
            .as_u64()
            .filter(|number| (1..=9_007_199_254_740_991).contains(number))
            .ok_or("配额必须为正安全整数")
    };
    if number("system_max_users")? < 101
        || number("tenant_max_users")? < 11 + number("import_headroom_per_tenant")?
    {
        return Err("配额必须覆盖管理员、固定普通身份及导入余量".into());
    }
    pacing(&value["pacing"])
}

fn database(value: &Value) -> Result<()> {
    json::exact(
        value,
        &[
            "python",
            "mysql_client",
            "server_uuid",
            "control",
            "targets",
        ],
        "数据库来源",
    )?;
    absolute(text(value, "python")?)?;
    absolute(text(value, "mysql_client")?)?;
    let uuid = text(value, "server_uuid")?;
    if uuid.len() != 36
        || !uuid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
    {
        return Err("数据库来源 UUID 无效".into());
    }
    connection(&value["control"])?;
    let mut keys = BTreeSet::new();
    for target in array(&value["targets"])? {
        json::exact(target, &["key", "mode", "connection"], "租户目标")?;
        if !identifier(text(target, "key")?, 32)
            || !keys.insert(text(target, "key")?)
            || !matches!(target["mode"].as_str(), Some("shared" | "dedicated"))
        {
            return Err("租户目标重复或无效".into());
        }
        connection(&target["connection"])?;
        if !text(&target["connection"], "password_env")?.starts_with("APP_") {
            return Err("目标密码引用必须由运行来源绑定".into());
        }
    }
    Ok(())
}

fn connection(value: &Value) -> Result<()> {
    json::exact(
        value,
        &[
            "host",
            "port",
            "database",
            "username",
            "password_env",
            "tls_mode",
        ],
        "数据库绑定",
    )?;
    let name = text(value, "database")?;
    if !matches!(value["host"].as_str(), Some("127.0.0.1" | "localhost"))
        || !value["port"]
            .as_u64()
            .is_some_and(|port| (1..=65535).contains(&port))
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || !matches!(value["tls_mode"].as_str(), Some("required" | "disabled"))
    {
        return Err("数据库必须是明确的本机隔离目标".into());
    }
    text(value, "username")?;
    secret_name(text(value, "password_env")?)
}

fn tenants(value: &Value) -> Result<()> {
    let tenants = array(&value["tenants"])?;
    if tenants.len() != 10 {
        return Err("必须登记十个普通租户".into());
    }
    let targets = array(&value["database"]["targets"])?;
    let mut ids = BTreeSet::new();
    let mut addresses = BTreeSet::from([text(&value["system_admin"], "client_address")?]);
    for (index, tenant) in tenants.iter().enumerate() {
        json::exact(
            tenant,
            &["slot", "tenant_id", "target_key", "admin"],
            "普通租户",
        )?;
        let id = text(tenant, "tenant_id")?;
        if tenant["slot"] != format!("tenant-{:02}", index + 1)
            || id == "system"
            || id.len() > 64
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || !ids.insert(id)
            || !targets
                .iter()
                .any(|target| target["key"] == tenant["target_key"])
        {
            return Err("实际十租户映射无效".into());
        }
        credential(&tenant["admin"], id)?;
        if !addresses.insert(text(&tenant["admin"], "client_address")?) {
            return Err("准备管理员客户地址必须独立".into());
        }
    }
    let mut physical = BTreeSet::new();
    for target in targets {
        let count = tenants
            .iter()
            .filter(|tenant| tenant["target_key"] == target["key"])
            .count();
        let connection = &target["connection"];
        if count == 0
            || (target["mode"] == "dedicated" && count != 1)
            || !physical.insert((
                connection["port"].as_u64(),
                text(connection, "database")?.to_lowercase(),
            ))
        {
            return Err("目标必须明确使用，不能重复物理库或共享独立库".into());
        }
    }
    Ok(())
}

fn credential(value: &Value, tenant: &str) -> Result<()> {
    json::exact(
        value,
        &["tenant_id", "username", "password_env", "client_address"],
        "准备管理员",
    )?;
    let address = text(value, "client_address")?;
    if value["tenant_id"] != tenant
        || text(value, "username")?.trim().is_empty()
        || !address.parse::<Ipv4Addr>().is_ok_and(|parsed| {
            parsed.to_string() == address
                && parsed.octets()[0] == 198
                && matches!(parsed.octets()[1], 18 | 19)
        })
    {
        return Err("准备管理员身份或固定地址无效".into());
    }
    secret_name(text(value, "password_env")?)
}

fn pacing(value: &Value) -> Result<()> {
    json::exact(value, &["contract", "bindings"], "请求节奏")?;
    let contract = &value["contract"];
    json::exact(
        contract,
        &[
            "authority_sha256",
            "request_interval_ms",
            "operation_intervals_ms",
            "sample_prepare_wait_ms",
        ],
        "节奏契约",
    )?;
    json::exact(
        &value["bindings"],
        &["python", "login_budget_state"],
        "节奏绑定",
    )?;
    let interval = |value: &Value, minimum| {
        value
            .as_u64()
            .is_some_and(|number| (minimum..=9_007_199_254_740_991).contains(&number))
    };
    if !sha256(&contract["authority_sha256"])
        || !interval(&contract["request_interval_ms"], 1)
        || !interval(&contract["sample_prepare_wait_ms"], 0)
    {
        return Err("节奏契约摘要或参数无效".into());
    }
    for (operation, milliseconds) in json::object(&contract["operation_intervals_ms"], "操作节奏")?
    {
        if operation.is_empty()
            || !operation.as_bytes()[0].is_ascii_lowercase()
            || !operation
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            || !interval(milliseconds, 1)
        {
            return Err("操作节奏参数无效".into());
        }
    }
    absolute(text(&value["bindings"], "python")?)?;
    absolute(text(&value["bindings"], "login_budget_state")?)
}

fn identifier(value: &str, maximum: usize) -> bool {
    (3..=maximum).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn secret_name(value: &str) -> Result<()> {
    if !value.as_bytes()[0].is_ascii_uppercase()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err("密码引用必须使用明确名称，不能包含密码值".into());
    }
    Ok(())
}

fn absolute(value: &str) -> Result<()> {
    if !Path::new(value).is_absolute() || value.contains(['\r', '\n', '\0']) {
        return Err("身份来源必须使用绝对路径".into());
    }
    Ok(())
}

fn endpoint(value: &str) -> Result<()> {
    let url = url::Url::parse(value)?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
        || url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("身份准备入口必须是无凭据的 loopback 根地址".into());
    }
    Ok(())
}

fn sha256(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
