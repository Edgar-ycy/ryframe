//! 固定身份池的纯模型；选择实际前 N 个绑定，并生成不含实际身份的收据。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

type SelectionResult = Result<Value, &'static str>;

const COUNT: &str = "身份池选中数量无效";
const LEGACY: &str = "旧 identities 结构不可使用，必须声明独立身份池";
const POOL: &str = "身份池必须固定角色权限摘要、逻辑槽位及客户地址模型";
const MISSING: &str = "身份池缺少实际选中的独立用户";
const BINDING: &str = "身份池槽位、固定地址或具体身份绑定无效";
const DISTINCT: &str = "实际选中身份必须为独立用户、逻辑槽位与固定地址";
const TENANT: &str = "逻辑租户与实际租户必须保持一一对应";
const DISTRIBUTION: &str = "实际前 N 身份的租户分布必须匹配固定负载声明";
const WORKFLOW: &str = "必须唯一声明当前身份池所属场景";

pub(crate) fn workload(config: &Value, suite: &str, name: &str, count: u64) -> SelectionResult {
    workflow_selection(config, suite, Some(&json!(name)), count)
}

pub(crate) fn homepage(config: &Value, cycles: u64) -> SelectionResult {
    pool_selection(
        config,
        &config["contract"]["homepage"]["identity_pool"],
        cycles,
    )
}

pub(crate) fn validate_suite(config: &Value, suite: &str, count: u64) -> SelectionResult {
    let selections = if suite == "homepage" {
        // 首页场景使用 contract.cycles，忽略传入的并发数。
        let cycles = safe_integer(&config["contract"]["cycles"]).ok_or(COUNT)?;
        let mut evidence = homepage(config, cycles)?["evidence"].clone();
        evidence["workflow"] = json!("homepage");
        vec![evidence]
    } else {
        let workflows = workflows(config, suite)
            .filter(|values| !values.is_empty())
            .ok_or("当前场景没有固定身份池")?;
        workflows
            .iter()
            .map(|workflow| {
                workflow
                    .get("name")
                    .and_then(Value::as_str)
                    .map_or_else(
                        || workflow_selection(config, suite, workflow.get("name"), count),
                        |name| workload(config, suite, name, count),
                    )
                    .map(|selected| selected["evidence"].clone())
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(json!({"format_version": 1, "suite": suite, "selections": selections}))
}

fn workflows<'a>(config: &'a Value, suite: &str) -> Option<&'a Vec<Value>> {
    config["contract"]["workloads"].get(suite)?.as_array()
}

fn workflow_selection(
    config: &Value,
    suite: &str,
    name: Option<&Value>,
    count: u64,
) -> SelectionResult {
    let rows = workflows(config, suite).ok_or(WORKFLOW)?;
    // 空工作流条目无效；其他 JSON 值缺失 name 时按缺失属性匹配。
    if rows.iter().any(Value::is_null) {
        return Err(WORKFLOW);
    }
    let mut matches = rows
        .iter()
        .filter(|row| strict_equal(row.get("name"), name));
    let selected = matches.next().ok_or(WORKFLOW)?;
    if matches.next().is_some() {
        return Err(WORKFLOW);
    }
    let mut result = pool_selection(config, &selected["identity_pool"], count)?;
    let distribution = result["evidence"]["tenant_distribution"]
        .as_object()
        .ok_or(DISTRIBUTION)?;
    let workflow_name = name.and_then(Value::as_str);
    if (suite == "tenants" || (suite == "jobs" && workflow_name == Some("import")))
        && (distribution.len() != 10
            || distribution.contains_key("system")
            || distribution
                .values()
                .any(|value| value.as_f64() != Some(count as f64 / 10.0)))
    {
        return Err("实际并发用户必须均匀覆盖十个普通租户");
    }
    if suite == "jobs"
        && workflow_name == Some("schedule")
        && (distribution.len() != 1 || distribution.get("system") != Some(&json!(count)))
    {
        return Err("当前调度目标只能使用 system 普通身份池");
    }
    if let Some(name) = name {
        result["evidence"]["workflow"] = name.clone();
    }
    Ok(result)
}

fn strict_equal(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(Value::Number(left)), Some(Value::Number(right))) => left.as_f64() == right.as_f64(),
        (Some(left @ (Value::Array(_) | Value::Object(_))), Some(right)) => {
            std::ptr::eq(left, right)
        }
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn pool_selection(config: &Value, pool_name: &Value, count: u64) -> SelectionResult {
    if !(1..=10_000).contains(&count) {
        return Err(COUNT);
    }
    if config["bindings"]
        .as_object()
        .is_some_and(|bindings| bindings.contains_key("identities"))
    {
        return Err(LEGACY);
    }
    let name = pool_name
        .as_str()
        .filter(|name| valid_name(name))
        .ok_or(POOL)?;
    let pool = &config["contract"]["identity_pools"][name];
    let bindings = &config["bindings"]["identity_pools"][name];
    if !exact(
        pool,
        &[
            "roles_sha256",
            "permissions_sha256",
            "client_address_model",
            "slots",
            "selections",
        ],
    ) || !digest(&pool["roles_sha256"])
        || !digest(&pool["permissions_sha256"])
        || pool["client_address_model"] != "fixed-benchmark-per-user"
        || !pool["selections"].is_object()
    {
        return Err(POOL);
    }
    let slots = pool["slots"].as_array().ok_or(POOL)?;
    let bindings = bindings.as_array().ok_or(POOL)?;
    let count_usize = usize::try_from(count).map_err(|_| COUNT)?;
    let slots = slots.get(..count_usize).ok_or(MISSING)?;
    let selected = bindings.get(..count_usize).ok_or(MISSING)?;
    let mut state = PoolState::default();
    let identities = slots
        .iter()
        .zip(selected)
        .map(|(slot, identity)| select_identity(slot, identity, &mut state))
        .collect::<Result<Vec<_>, _>>()?;
    state.validate(&pool["selections"][count.to_string()])?;
    Ok(json!({
        "identities": identities,
        "evidence": {
            "identity_pool": name,
            "count": count,
            "tenant_distribution": state.distribution,
            "roles_sha256": pool["roles_sha256"],
            "permissions_sha256": pool["permissions_sha256"],
            "slots": slots,
        },
    }))
}

#[derive(Default)]
struct PoolState {
    tenants: BTreeMap<String, String>,
    reverse_tenants: BTreeMap<String, String>,
    distribution: BTreeMap<String, u64>,
    users: BTreeSet<String>,
    addresses: BTreeSet<String>,
    subjects: BTreeSet<(String, String)>,
}

impl PoolState {
    fn add(
        &mut self,
        tenant_slot: &str,
        user_slot: &str,
        address: &str,
        tenant: &str,
        username: &str,
    ) -> Result<(), &'static str> {
        let subject = (tenant.to_owned(), js_trim(username).to_lowercase());
        if self.users.contains(user_slot)
            || self.addresses.contains(address)
            || self.subjects.contains(&subject)
        {
            return Err(DISTINCT);
        }
        if (tenant_slot == "system") != (tenant == "system")
            || self
                .tenants
                .get(tenant_slot)
                .is_some_and(|value| value != tenant)
            || self
                .reverse_tenants
                .get(tenant)
                .is_some_and(|value| value != tenant_slot)
        {
            return Err(TENANT);
        }
        self.tenants
            .insert(tenant_slot.to_owned(), tenant.to_owned());
        self.reverse_tenants
            .insert(tenant.to_owned(), tenant_slot.to_owned());
        *self.distribution.entry(tenant_slot.to_owned()).or_default() += 1;
        self.users.insert(user_slot.to_owned());
        self.addresses.insert(address.to_owned());
        self.subjects.insert(subject);
        Ok(())
    }

    fn validate(&self, declared: &Value) -> Result<(), &'static str> {
        let declared = declared.as_object().ok_or(DISTRIBUTION)?;
        if declared.len() != self.tenants.len() {
            return Err(DISTRIBUTION);
        }
        for (tenant, amount) in declared {
            let amount = safe_integer(amount).filter(|amount| *amount >= 1);
            if !valid_name(tenant)
                || amount.is_none()
                || amount != self.distribution.get(tenant).copied()
            {
                return Err(DISTRIBUTION);
            }
        }
        Ok(())
    }
}

fn select_identity(slot: &Value, identity: &Value, state: &mut PoolState) -> SelectionResult {
    if !exact(slot, &["tenant_slot", "user_slot", "client_address"])
        || !exact(identity, &["tenant_id", "username", "password_env"])
    {
        return Err(BINDING);
    }
    let tenant_slot = text(slot, "tenant_slot")?;
    let user_slot = text(slot, "user_slot")?;
    let address = text(slot, "client_address")?;
    let tenant = text(identity, "tenant_id")?;
    let username = text(identity, "username")?;
    let password = text(identity, "password_env")?;
    if !valid_name(tenant_slot)
        || !valid_name(user_slot)
        || !valid_address(address)
        || tenant.is_empty()
        || tenant.len() > 64
        || !tenant
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || js_trim(username).is_empty()
        || username.encode_utf16().count() > 64
        || password.is_empty()
        || !password.as_bytes()[0].is_ascii_uppercase()
        || !password
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(BINDING);
    }
    state.add(tenant_slot, user_slot, address, tenant, username)?;
    let mut selected = identity.clone();
    selected["client_address"] = json!(address);
    Ok(selected)
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, &'static str> {
    value[key].as_str().ok_or(BINDING)
}

fn exact(value: &Value, fields: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field))
    })
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn digest(value: &Value) -> bool {
    value.as_str().is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn valid_address(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 4
        && parts[0] == "198"
        && matches!(parts[1], "18" | "19")
        && parts[2..].iter().all(|part| {
            part.parse::<u8>()
                .is_ok_and(|number| number.to_string() == *part)
        })
}

fn safe_integer(value: &Value) -> Option<u64> {
    let value = value.as_f64()?;
    (value.is_finite() && (0.0..=9_007_199_254_740_991.0).contains(&value) && value.fract() == 0.0)
        .then_some(value as u64)
}

fn js_trim(value: &str) -> &str {
    value.trim_matches(|character| {
        matches!(
            character,
            '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
            '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
            '\u{205f}' | '\u{3000}' | '\u{feff}'
        )
    })
}
