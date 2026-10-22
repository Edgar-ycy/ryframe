use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::error;
use crate::{
    devex::runtime_pacing,
    http_session::{RequestMetadata, SessionError},
};

// 稳态时刻和间隔均保留纳秒；配置及证据的公开单位仍为毫秒。
pub(crate) const NS_PER_MS: u64 = 1_000_000;

pub(crate) struct Identity {
    pub(crate) tenant: String,
    pub(crate) username: String,
    pub(crate) address: String,
}

pub(crate) struct Pacer {
    pacing: Value,
    authority: Value,
    catalog: BTreeMap<String, Value>,
    identity: Identity,
    starts: BTreeMap<String, u64>,
}

pub(crate) struct Plan {
    pub(crate) keys: Vec<(String, u64)>,
    pub(crate) wait_ns: u64,
}

impl Pacer {
    pub(crate) fn new(
        pacing: &Value,
        authority: &Value,
        catalog: BTreeMap<String, Value>,
        identity: &Value,
    ) -> Result<Self, SessionError> {
        runtime_pacing::validate_authority(pacing, authority, &catalog)
            .map_err(|_| error("pacing-contract"))?;
        let tenant = identity["tenant_id"]
            .as_str()
            .ok_or_else(|| error("pacing-identity"))?;
        let username = identity["username"]
            .as_str()
            .ok_or_else(|| error("pacing-identity"))?;
        let address = identity["client_address"]
            .as_str()
            .ok_or_else(|| error("pacing-identity"))?;
        if !benchmark_address(address) {
            return Err(error("pacing-identity"));
        }
        Ok(Self {
            pacing: pacing.clone(),
            authority: authority.clone(),
            catalog,
            identity: Identity {
                tenant: tenant.into(),
                username: username.into(),
                address: address.into(),
            },
            starts: BTreeMap::new(),
        })
    }

    pub(crate) fn pacing(&self) -> &Value {
        &self.pacing
    }
    pub(crate) fn identity(&self) -> &Identity {
        &self.identity
    }
    pub(crate) fn login_window(&self) -> u64 {
        number(&self.authority["login"]["window_ms"])
    }

    pub(crate) fn plan(&self, metadata: &RequestMetadata, now: u64) -> Result<Plan, SessionError> {
        let operation = self
            .catalog
            .get(&metadata.operation)
            .ok_or_else(|| error("pacing-operation"))?;
        if operation["method"].as_str() != Some(metadata.method.as_str())
            || operation["path"].as_str() != Some(metadata.route.as_str())
        {
            return Err(error("pacing-operation"));
        }
        let address = &self.identity.address;
        let principal = serde_json::to_string(&json!([
            self.identity.tenant,
            self.identity
                .username
                .trim_matches(js_whitespace)
                .to_lowercase(),
        ]))
        .map_err(|_| error("pacing-identity"))?;
        let request = number(&self.pacing["request_interval_ms"]);
        let declared = self.pacing["operation_intervals_ms"].get(&metadata.operation);
        let interval = request.max(declared.map_or(0, number));
        let rule = runtime_pacing::matched_rule(operation, &self.authority["api"]["rules"]);
        let mut keys = vec![(format!("ip:{address}"), request)];
        if declared.is_some() {
            keys.push((
                format!("operation:{address}:{}", metadata.operation),
                interval,
            ));
        }
        if self.authority["user"]["enabled"] == true {
            keys.push((format!("user:{principal}"), request));
        }
        if let Some(rule) = rule.filter(|rule| !rule.is_empty()) {
            keys.push((format!("rule:{address}:{rule}"), interval));
        }
        if metadata.operation == "post_auth_login" {
            keys.push((format!("login:{principal}"), interval));
            keys.push((format!("login-ip:{address}"), interval));
        }
        let mut wait_ns = 0;
        for (key, gap) in &keys {
            if let Some(started) = self.starts.get(key) {
                if now < *started {
                    return Err(error("pacing-clock"));
                }
                let gap = gap
                    .checked_mul(NS_PER_MS)
                    .ok_or_else(|| error("pacing-clock"))?;
                let deadline = started
                    .checked_add(gap)
                    .ok_or_else(|| error("pacing-clock"))?;
                wait_ns = wait_ns.max(deadline.saturating_sub(now));
            }
        }
        Ok(Plan { keys, wait_ns })
    }

    pub(crate) fn sent(
        &mut self,
        plan: &Plan,
        before: u64,
        finished: u64,
        started: u64,
    ) -> Result<(), SessionError> {
        let deadline = before
            .checked_add(plan.wait_ns)
            .ok_or_else(|| error("pacing-clock"))?;
        if finished < deadline || started < finished {
            return Err(error("pacing-clock"));
        }
        for (key, _) in &plan.keys {
            self.starts.insert(key.clone(), started);
        }
        Ok(())
    }
}

pub(crate) fn binding(value: &Value) -> Result<PathBuf, SessionError> {
    let fields = value.as_object().ok_or_else(|| error("pacing-binding"))?;
    if fields.len() != 2
        || !fields.contains_key("python")
        || !fields.contains_key("login_budget_state")
    {
        return Err(error("pacing-binding"));
    }
    for name in ["python", "login_budget_state"] {
        let value = fields[name]
            .as_str()
            .ok_or_else(|| error("pacing-binding"))?;
        if !Path::new(value).is_absolute() || value.contains(['\r', '\n', '\0']) {
            return Err(error("pacing-binding"));
        }
    }
    Ok(PathBuf::from(
        fields["login_budget_state"]
            .as_str()
            .ok_or_else(|| error("pacing-binding"))?,
    ))
}

fn benchmark_address(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 4
        && parts[0] == "198"
        && matches!(parts[1], "18" | "19")
        && parts[2..].iter().all(|part| {
            !part.is_empty()
                && part.len() <= 3
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() < 3 || !part.starts_with('0'))
                && part.parse::<u16>().is_ok_and(|value| value <= 255)
        })
}

fn js_whitespace(value: char) -> bool {
    matches!(value, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}'
        | '\u{3000}' | '\u{feff}')
}

fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| value.as_f64().unwrap_or(0.0) as u64)
}

pub(crate) struct Wait {
    previous: u64,
    deadline: u64,
}

impl Wait {
    pub(crate) fn new(before: u64, requested: u64) -> Result<Self, SessionError> {
        Ok(Self {
            previous: before,
            deadline: before
                .checked_add(requested)
                .ok_or_else(|| error("pacing-clock"))?,
        })
    }

    pub(crate) fn remaining(&mut self, now: u64) -> Result<Option<u64>, SessionError> {
        if now < self.previous {
            return Err(error("pacing-clock"));
        }
        self.previous = now;
        Ok((now < self.deadline).then(|| self.deadline - now))
    }
}

pub(crate) fn budget_deadline(now: u64, window: u64) -> Result<u64, SessionError> {
    window
        .checked_mul(3)
        .and_then(|value| value.checked_add(30_000))
        .and_then(|value| value.checked_mul(NS_PER_MS))
        .and_then(|value| now.checked_add(value))
        .ok_or_else(|| error("pacing-clock"))
}

pub(crate) fn budget_wait(now: u64, requested: u64, deadline: u64) -> Result<(), SessionError> {
    if requested == 0
        || requested
            .checked_mul(NS_PER_MS)
            .and_then(|value| now.checked_add(value))
            .is_none_or(|end| end > deadline)
    {
        return Err(error("pacing-budget-timeout"));
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct Summary {
    preparation: BTreeMap<String, [u64; 3]>,
    budget: [u64; 3],
}

impl Summary {
    pub(crate) fn fixed(&mut self, operation: &str, requested: u64, duration: u64) {
        let row = self.preparation.entry(operation.into()).or_default();
        row[0] = row[0].saturating_add(1);
        row[1] = row[1].saturating_add(requested);
        row[2] = row[2].saturating_add(duration);
    }
    pub(crate) fn budget_wait(&mut self, requested: u64) {
        self.budget[0] = self.budget[0].saturating_add(1);
        self.budget[1] = self.budget[1].saturating_add(requested);
    }
    pub(crate) fn budget_duration(&mut self, duration: u64) {
        self.budget[2] = self.budget[2].saturating_add(duration);
    }
    pub(crate) fn document(&self, pacing: &Value) -> Value {
        let preparation: BTreeMap<_, _> = self.preparation.iter().map(|(name, row)| {
            (name, json!({"requests": row[0], "requested_ms": milliseconds(row[1]), "duration_ms": milliseconds(row[2])}))
        }).collect();
        json!({"format_version": 1, "pacing": pacing, "measurement": {}, "preparation": preparation,
            "login_budget": {"waits": self.budget[0], "requested_ms": self.budget[1], "duration_ms": milliseconds(self.budget[2])}})
    }
}

pub(crate) fn milliseconds(nanoseconds: u64) -> Value {
    if nanoseconds.is_multiple_of(NS_PER_MS) {
        json!(nanoseconds / NS_PER_MS)
    } else {
        json!(nanoseconds as f64 / NS_PER_MS as f64)
    }
}
