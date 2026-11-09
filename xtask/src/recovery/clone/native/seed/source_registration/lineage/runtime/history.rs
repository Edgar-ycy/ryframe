use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

use regex::Regex;
use serde_json::{Value, json};

use super::{artifact, contract, documents, identity, positive_integer, regular};
use crate::Result;

#[derive(Default)]
pub(super) struct History {
    pub(super) latest: BTreeMap<String, Value>,
    pub(super) generations: Vec<Value>,
}

struct Intent {
    role: String,
    started: Option<Value>,
    failed: bool,
    closed: bool,
    log: String,
}

#[derive(Default)]
struct Machine {
    intents: BTreeMap<String, Intent>,
    identities: HashMap<(String, String), String>,
    history: History,
}

pub(super) fn collect(
    backend: &Path,
    runtime: &Path,
    contract: &Value,
    value: &Value,
    pending: Option<(&str, &str)>,
) -> Result<History> {
    documents::exact(
        value,
        &[
            "format_version",
            "kind",
            "scope_id",
            "runtime_directory",
            "events",
        ],
        "生产者历史",
    )?;
    let events = documents::array(&value["events"], "生产者历史事件")?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-clone-producer-history"
        || value["scope_id"] != contract["scope_id"]
        || Path::new(documents::text(
            &value["runtime_directory"],
            "历史runtime目录",
        )?) != runtime
        || events.is_empty()
    {
        return Err("生产者历史与明确runtime不一致".into());
    }
    let pending = pending.filter(|(role, token)| {
        events.last().is_some_and(|event| {
            event["event"] == "start-intent" && event["role"] == *role && event["token"] == *token
        })
    });
    let mut machine = Machine::default();
    let mut previous_time = String::new();
    for (sequence, event) in events.iter().enumerate() {
        let kind = event_shape(event)?;
        let time = positive_integer(&event["time_ns"])?;
        let role = documents::text(&event["role"], "生产者角色")?;
        if event["sequence"].as_u64() != Some(sequence as u64)
            || !matches!(role, "api" | "worker")
            || (time.len(), time.as_str()) < (previous_time.len(), previous_time.as_str())
        {
            return Err("生产者历史序号、时间或角色无效".into());
        }
        previous_time = time;
        match kind {
            "start-intent" => {
                let future_log = pending
                    .is_some_and(|(role, token)| event["role"] == role && event["token"] == token);
                machine.intent(backend, runtime, contract, event, future_log)?;
            }
            "started" => machine.started(contract, event)?,
            "start-failed" => machine.failed(event)?,
            _ => machine.close(contract, event)?,
        }
    }
    if machine.intents.iter().any(|(token, intent)| {
        intent.started.is_none()
            && !intent.failed
            && !pending.is_some_and(|(role, allowed)| {
                allowed == token.as_str() && role == intent.role.as_str()
            })
    }) {
        return Err("生产者启动意图没有started或start-failed收据".into());
    }
    Ok(machine.history)
}

fn event_shape(event: &Value) -> Result<&str> {
    let kind = documents::text(&event["event"], "生产者事件")?;
    let mut fields = vec!["sequence", "time_ns", "event", "role"];
    fields.extend_from_slice(match kind {
        "start-intent" => &[
            "token",
            "artifact",
            "ready_url",
            "configuration_sha256",
            "log",
        ][..],
        "started" => &["token", "identity"][..],
        "start-failed" => &["token", "error_type", "log"][..],
        "startup-rollback" | "stopped" => &["identity"][..],
        _ => return Err("生产者历史包含未知事件".into()),
    });
    documents::exact(event, &fields, "生产者事件")?;
    Ok(kind)
}

impl Machine {
    fn intent(
        &mut self,
        backend: &Path,
        runtime: &Path,
        receipt: &Value,
        event: &Value,
        future_log: bool,
    ) -> Result<()> {
        let token = documents::text(&event["token"], "启动token")?;
        let role = documents::text(&event["role"], "启动角色")?;
        if !Regex::new(r"^[a-f0-9]{32}$")?.is_match(token) || self.intents.contains_key(token) {
            return Err("生产者启动token无效或重复".into());
        }
        let artifact = artifact(backend, &event["artifact"])?;
        let log = documents::text(&event["log"], "生产者日志名")?;
        if artifact != receipt["artifacts"][role]
            || event["configuration_sha256"] != receipt["configuration_sha256"]
            || log != format!("{role}-{token}.log")
        {
            return Err("生产者启动意图与runtime产物或配置不一致".into());
        }
        contract::ready_url(&event["ready_url"], false)?;
        let log_path = runtime.join(log);
        let not_created = future_log
            && std::fs::symlink_metadata(&log_path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        if !not_created {
            regular(&log_path, runtime, true)?;
        }
        self.intents.insert(
            token.to_owned(),
            Intent {
                role: role.to_owned(),
                started: None,
                failed: false,
                closed: false,
                log: log.to_owned(),
            },
        );
        Ok(())
    }

    fn started(&mut self, receipt: &Value, event: &Value) -> Result<()> {
        let token = documents::text(&event["token"], "started token")?;
        let role = documents::text(&event["role"], "started角色")?;
        let intent = self
            .intents
            .get_mut(token)
            .ok_or("started事件没有对应启动意图")?;
        if intent.role != role || intent.started.is_some() || intent.failed {
            return Err("started事件没有唯一对应的启动意图".into());
        }
        let identity = identity(&event["identity"], &receipt["artifacts"][role])?;
        let key = identity_key(&identity)?;
        if self.identities.insert(key, token.to_owned()).is_some() {
            return Err("同一runtime创建身份被多个启动token重复登记".into());
        }
        intent.started = Some(identity.clone());
        self.history
            .latest
            .insert(role.to_owned(), identity.clone());
        self.history
            .generations
            .push(json!({"sequence": event["sequence"], "role": role,
            "token": token, "identity": identity}));
        Ok(())
    }

    fn failed(&mut self, event: &Value) -> Result<()> {
        let token = documents::text(&event["token"], "失败token")?;
        let role = documents::text(&event["role"], "失败角色")?;
        let intent = self
            .intents
            .get_mut(token)
            .ok_or("启动失败事件没有对应启动意图")?;
        let error_type = documents::text(&event["error_type"], "启动错误类型")?;
        if intent.role != role
            || intent.failed
            || intent.closed
            || event["log"] != intent.log
            || !Regex::new(r"^[A-Za-z_][A-Za-z0-9_.]*$")?.is_match(error_type)
        {
            return Err("启动失败事件没有唯一对应的启动意图".into());
        }
        intent.failed = true;
        if intent.started.is_some() {
            intent.closed = true;
        }
        Ok(())
    }

    fn close(&mut self, receipt: &Value, event: &Value) -> Result<()> {
        let role = documents::text(&event["role"], "结束角色")?;
        let identity = identity(&event["identity"], &receipt["artifacts"][role])?;
        let token = self
            .identities
            .get(&identity_key(&identity)?)
            .ok_or("结束事件缺少已启动代次")?;
        let intent = self.intents.get_mut(token).ok_or("结束事件缺少启动意图")?;
        if intent.role != role
            || intent.started.as_ref() != Some(&identity)
            || intent.failed
            || intent.closed
        {
            return Err("停止或回滚事件没有唯一对应的已启动代次".into());
        }
        intent.closed = true;
        Ok(())
    }
}

fn identity_key(identity: &Value) -> Result<(String, String)> {
    Ok((
        positive_integer(&identity["pid"])?,
        documents::text(&identity["started"], "创建时刻")?.to_owned(),
    ))
}
