//! 保留原生产者历史字段与日志名；未知启动不伪造失败关闭事件。
use super::input::Frame;
use crate::{
    Result,
    recovery::{
        clone::native::{
            json as document, post_process,
            resources::common,
            seed::source_registration::lineage::runtime::{
                collect_for_product, validate_history_projection,
            },
        },
        full_stack::worker::role::Role,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn intent(frame: &Frame, token: &str) -> Result<()> {
    let url = api_ready(frame)?;
    Producer::new(
        &frame.backend,
        &frame.post.runtime,
        &frame.runtime,
        Role::Api,
        token,
        &url,
    )?
    .intent()
}

pub(super) fn started(frame: &Frame, token: &str, identity: &Value) -> Result<()> {
    let url = api_ready(frame)?;
    Producer::new(
        &frame.backend,
        &frame.post.runtime,
        &frame.runtime,
        Role::Api,
        token,
        &url,
    )?
    .started(identity)
}

fn api_ready(frame: &Frame) -> Result<String> {
    Ok(format!(
        "{}/readyz",
        document::text(&frame.selected["api_url"], "API地址")?.trim_end_matches('/')
    ))
}

/// 由持有本运行控制器的调用方在原角色操作前后发布；收尾前必须核验原树。
pub(in crate::recovery::clone::native) struct Producer<'a> {
    backend: &'a Path,
    directory: &'a Path,
    runtime: &'a Value,
    role: &'static str,
    token: &'a str,
    ready_url: &'a str,
}

impl<'a> Producer<'a> {
    pub(in crate::recovery::clone::native) fn new(
        backend: &'a Path,
        directory: &'a Path,
        runtime: &'a Value,
        role: Role,
        token: &'a str,
        ready_url: &'a str,
    ) -> Result<Self> {
        if role == Role::Frontend
            || token.len() != 32
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || role == Role::Worker && runtime["worker_ready_url"] != ready_url
        {
            return Err("生产者角色、原操作编号或就绪地址与运行合同不同".into());
        }
        Ok(Self {
            backend,
            directory,
            runtime,
            role: role.name(),
            token,
            ready_url,
        })
    }

    pub(in crate::recovery::clone::native) fn intent(&self) -> Result<()> {
        self.publish(
            json!({"event":"start-intent","role":self.role,"token":self.token,
            "artifact":self.runtime["artifacts"][self.role],"ready_url":self.ready_url,
            "configuration_sha256":self.runtime["configuration_sha256"],"log":self.log()}),
        )
    }

    pub(in crate::recovery::clone::native) fn started(&self, identity: &Value) -> Result<()> {
        self.publish(
            json!({"event":"started","role":self.role,"token":self.token,
            "identity":identity}),
        )
    }

    pub(in crate::recovery::clone::native) fn failed(
        &self,
        error_type: &str,
        prove_closed: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        prove_closed()?;
        self.publish(
            json!({"event":"start-failed","role":self.role,"token":self.token,
            "error_type":error_type,"log":self.log()}),
        )
    }

    pub(in crate::recovery::clone::native) fn rollback(
        &self,
        identity: &Value,
        prove_closed: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        prove_closed()?;
        self.publish(json!({"event":"startup-rollback","role":self.role,"identity":identity}))
    }

    pub(in crate::recovery::clone::native) fn stopped(
        &self,
        identity: &Value,
        prove_closed: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        prove_closed()?;
        self.publish(json!({"event":"stopped","role":self.role,"identity":identity}))
    }

    fn log(&self) -> String {
        format!("{}-{}.log", self.role, self.token)
    }

    fn publish(&self, event: Value) -> Result<()> {
        let runtime_path = self.directory.join("runtime.json");
        let contract = common::binding(&common::common_file(
            self.backend,
            &runtime_path,
            "生产者原运行合同",
        )?)?;
        let product = post_process::runtime::product_root(self.backend, self.runtime)?;
        if post_process::runtime::contract(self.backend, &product, self.directory)? != *self.runtime
        {
            return Err("生产者原运行合同发生变化".into());
        }
        let path = self.directory.join("producer-history.json");
        let (before, mut history) = load(self.backend, self.directory, self.runtime, &path)?;
        let events = history["events"]
            .as_array()
            .ok_or("生产者历史必须是事件数组")?;
        let duplicate = self.already_recorded(events, &event)?;
        if !duplicate {
            append(&mut history, event)?;
        }
        let pending = history["events"]
            .as_array()
            .and_then(|events| events.last())
            .filter(|last| {
                last["event"] == "start-intent"
                    && last["role"] == self.role
                    && last["token"] == self.token
            })
            .map(|_| (self.role, self.token));
        validate_history_projection(
            self.backend,
            self.directory,
            self.runtime,
            &history,
            pending,
            duplicate,
        )?;
        common::bound_file(self.backend, &contract, "生产者原运行合同后像")?;
        if duplicate {
            let original = before.as_ref().ok_or("重复生产者事件缺少原历史")?;
            common::bound_file(self.backend, original, "原已记录生产者事件后像")?;
            return Ok(());
        }
        store(self.backend, &path, before.as_ref(), &history)
    }

    fn already_recorded(&self, events: &[Value], event: &Value) -> Result<bool> {
        let kind = document::text(&event["event"], "生产者事件")?;
        let uses_identity = matches!(kind, "startup-rollback" | "stopped");
        if events.iter().any(|record| {
            record["event"] == "start-intent"
                && record["token"] == self.token
                && (record["role"] != self.role || record["ready_url"] != self.ready_url)
        }) {
            return Err("生产者原启动意图的角色或就绪地址不同".into());
        }
        if uses_identity
            && !events.iter().any(|record| {
                record["event"] == "started"
                    && record["role"] == self.role
                    && record["token"] == self.token
                    && record["identity"] == event["identity"]
            })
        {
            return Err("生产者收尾身份不属于指定的原角色和启动编号".into());
        }
        let prior = events.iter().find(|record| {
            record["event"] == kind
                && record["role"] == self.role
                && if uses_identity {
                    record["identity"] == event["identity"]
                } else {
                    record["token"] == self.token
                }
        });
        if let Some(prior) = prior {
            let mut body = prior.as_object().ok_or("原生产者事件无效")?.clone();
            body.remove("sequence");
            body.remove("time_ns");
            if Value::Object(body) != *event {
                return Err("已记录的生产者事件与本次证据不同".into());
            }
            return Ok(true);
        }
        let last = events.iter().rfind(|record| record["role"] == self.role);
        let same_token =
            |record: &Value, event: &str| record["event"] == event && record["token"] == self.token;
        let valid = match kind {
            "start-intent" => last.is_none_or(|record| record["event"] != "started"),
            "started" => events.last().is_some_and(|record| {
                record["role"] == self.role && same_token(record, "start-intent")
            }),
            "start-failed" => events.last().is_some_and(|record| {
                record["role"] == self.role && same_token(record, "start-intent")
            }),
            _ => last.is_some_and(|record| same_token(record, "started")),
        };
        if !valid {
            return Err("生产者事件缺少当前原启动意图或角色前像".into());
        }
        Ok(false)
    }
}

fn event(
    backend: &Path,
    directory: &Path,
    runtime: &Value,
    expected: Option<&Value>,
    event: Value,
) -> Result<()> {
    let path = directory.join("producer-history.json");
    let (before, mut history) = load(backend, directory, runtime, &path)?;
    if expected.is_some_and(|expected| before.as_ref() != Some(expected)) {
        return Err("API生产者历史在完整树关闭期间变化，拒绝发布停止事件".into());
    }
    append(&mut history, event)?;
    store(backend, &path, before.as_ref(), &history)
}

fn load(
    backend: &Path,
    directory: &Path,
    runtime: &Value,
    path: &Path,
) -> Result<(Option<Value>, Value)> {
    let before = match std::fs::symlink_metadata(path) {
        Ok(_) => Some(common::binding(&common::common_file(
            backend,
            path,
            "API生产者历史",
        )?)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let history = if let Some(binding) = &before {
        common::read_bound(backend, binding, "API生产者历史")?
    } else {
        json!({"format_version":1,"kind":"devex-clone-producer-history",
            "scope_id":runtime["scope_id"],"runtime_directory":directory,"events":[]})
    };
    document::exact(
        &history,
        &[
            "format_version",
            "kind",
            "scope_id",
            "runtime_directory",
            "events",
        ],
        "API生产者历史",
    )?;
    if history["format_version"].as_u64() != Some(1)
        || history["kind"] != "devex-clone-producer-history"
        || history["scope_id"] != runtime["scope_id"]
        || history["runtime_directory"] != json!(directory)
    {
        return Err("API生产者历史与当前运行目录不一致，拒绝覆盖".into());
    }
    Ok((before, history))
}

pub(in crate::recovery::clone::native) fn append(
    history: &mut Value,
    mut event: Value,
) -> Result<()> {
    let events = history["events"]
        .as_array_mut()
        .ok_or("API生产者历史必须是事件数组")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    event["sequence"] = json!(events.len());
    event["time_ns"] = document::parse_fragment(&now.to_string(), "生产者事件实际时间")?;
    if events.last().is_some_and(|last| {
        last["time_ns"]
            .as_u64()
            .is_none_or(|value| u128::from(value) > now)
    }) {
        return Err("原生产者历史时间无效或晚于当前实际时刻".into());
    }
    events.push(event);
    Ok(())
}

fn store(backend: &Path, path: &Path, before: Option<&Value>, history: &Value) -> Result<()> {
    if let Some(binding) = before {
        common::bound_file(backend, binding, "API生产者历史写前绑定")?;
        document::write_replace(path, history)
    } else {
        document::write_new(path, history)
    }
}

/// 原角色最近一次尚未在生产者历史中关闭的启动。
pub(in crate::recovery::clone::native) struct Active {
    pub token: String,
    pub identity: Value,
    pub ready_url: String,
}

pub(in crate::recovery::clone::native) struct PendingStart {
    pub role: Role,
    pub token: String,
    pub ready_url: String,
    pub binding: Value,
}

/// 只定位未结束的原意图，不以它证明角色曾启动、失败或停止。
pub(in crate::recovery::clone::native) fn pending_start(
    backend: &Path,
    directory: &Path,
    runtime: &Value,
) -> Result<Option<PendingStart>> {
    let path = directory.join("producer-history.json");
    let (binding, history) = load(backend, directory, runtime, &path)?;
    let Some(binding) = binding else {
        return Ok(None);
    };
    let events = document::array(&history["events"], "seed 原生产者历史")?;
    let Some(last) = events
        .last()
        .filter(|event| event["event"] == "start-intent")
    else {
        return Ok(None);
    };
    let role = match last["role"].as_str() {
        Some("api") => Role::Api,
        Some("worker") => Role::Worker,
        _ => return Err("未结束的启动意图缺少已登记角色".into()),
    };
    let token = document::text(&last["token"], "待定启动预留")?;
    let ready_url = document::text(&last["ready_url"], "待定启动就绪地址")?;
    validate_history_projection(
        backend,
        directory,
        runtime,
        &history,
        Some((role.name(), token)),
        true,
    )?;
    common::bound_file(backend, &binding, "待定启动历史后像")?;
    Ok(Some(PendingStart {
        role,
        token: token.to_owned(),
        ready_url: ready_url.to_owned(),
        binding,
    }))
}

/// 只选已绑定的历史代次。调用方仍须在角色控制前核对原树和内核身份。
pub(in crate::recovery::clone::native) fn active(
    backend: &Path,
    product: &Path,
    directory: &Path,
    role: Role,
) -> Result<Option<Active>> {
    if role == Role::Frontend {
        return Err("生产者历史仅支持 API 和 Worker".into());
    }
    common::local_directory(backend, directory, "生产者运行目录")?;
    let path = directory.join("producer-history.json");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return without_history(backend, product, directory);
        }
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let collected = collect_for_product(backend, product, directory)?;
    let history = common::read_bound(backend, &collected["history"], "完整生产者历史")?;
    let events = document::array(&history["events"], "完整生产者事件")?;
    let role = role.name();
    let selected = match events.iter().rfind(|event| event["role"] == role) {
        Some(event) if event["event"] == "started" => {
            Some(active_started(backend, &collected, events, role, event)?)
        }
        Some(event)
            if matches!(
                event["event"].as_str(),
                Some("start-failed" | "startup-rollback" | "stopped")
            ) =>
        {
            None
        }
        None => None,
        _ => return Err("生产者历史存在未核实的启动意图".into()),
    };
    common::bound_file(backend, &collected["history"], "完整生产者历史后像")?;
    common::bound_file(backend, &collected["runtime"], "生产者运行合同后像")?;
    Ok(selected)
}

fn without_history(backend: &Path, product: &Path, directory: &Path) -> Result<Option<Active>> {
    post_process::runtime::contract(backend, product, directory)?;
    for entry in std::fs::read_dir(directory)? {
        let name = entry?.file_name();
        // 已由运行合同核对的 Worker 配置目录是启动前输入。
        if name == "worker-config" {
            continue;
        }
        let name = name
            .to_str()
            .ok_or("生产者运行文件名无效")?
            .to_ascii_lowercase();
        if name == "producer-history.json"
            || ["api", "worker"].into_iter().any(|role| {
                name.strip_prefix(role).is_some_and(|suffix| {
                    suffix == ".json" || suffix.starts_with('-') && suffix != "-control.guard"
                })
            })
        {
            return Err("运行目录有角色证据但缺少完整生产者历史".into());
        }
    }
    Ok(None)
}

fn active_started(
    backend: &Path,
    collected: &Value,
    events: &[Value],
    role: &str,
    event: &Value,
) -> Result<Active> {
    let token = document::text(&event["token"], "原启动编号")?;
    let intent = events
        .iter()
        .find(|record| {
            record["event"] == "start-intent" && record["role"] == role && record["token"] == token
        })
        .ok_or("已启动角色缺少原启动意图")?;
    let ready_url = document::text(&intent["ready_url"], "原就绪地址")?;
    let identity = common::validate_process_identity(&event["identity"])?;
    let receipt = document::array(&collected["role_receipts"], "已验证角色收据")?
        .iter()
        .find(|row| row["role"] == role && row["identity"] == identity)
        .ok_or("已启动身份不同于同角色实际收据")?;
    common::bound_file(backend, &receipt["receipt"], "原角色收据后像")?;
    Ok(Active {
        token: token.to_owned(),
        identity,
        ready_url: ready_url.to_owned(),
    })
}

pub(super) struct Closing {
    pub binding: Option<Value>,
    pub identity: Option<Value>,
}

pub(super) fn closing(backend: &Path, product: &Path, directory: &Path) -> Result<Closing> {
    let path = directory.join("producer-history.json");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Closing {
                binding: None,
                identity: None,
            });
        }
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    // 复用六文件发布的完整历史状态机；不另写较弱的事件解码器。
    let collected =
        crate::recovery::clone::native::seed::source_registration::lineage::runtime::collect_for_product(
            backend, product, directory,
        )?;
    let binding = common::binding(&path)?;
    if collected["history"] != binding {
        return Err("API生产者历史在完整代次核验期间变化".into());
    }
    let history = common::read_bound(backend, &binding, "API完整生产者历史")?;
    let latest = document::array(&history["events"], "API完整事件")?
        .iter()
        .rfind(|event| event["role"] == "api");
    let identity = match latest {
        Some(event) if event["event"] == "started" => {
            Some(common::validate_process_identity(&event["identity"])?)
        }
        Some(event)
            if matches!(
                event["event"].as_str(),
                Some("stopped" | "startup-rollback" | "start-failed")
            ) =>
        {
            None
        }
        None => None,
        _ => return Err("API生产者历史未形成完整已启动代次，保留未知现场".into()),
    };
    common::bound_file(backend, &binding, "API完整生产者历史后像")?;
    Ok(Closing {
        binding: Some(binding),
        identity,
    })
}

pub(super) fn closed(
    backend: &Path,
    directory: &Path,
    runtime: &Value,
    closing: &Closing,
) -> Result<()> {
    if let Some(binding) = &closing.binding {
        common::bound_file(backend, binding, "API停止后完整历史绑定")?;
    } else {
        super::guard::absent(&directory.join("producer-history.json"))?;
    }
    let Some(identity) = &closing.identity else {
        return Ok(());
    };
    event(
        backend,
        directory,
        runtime,
        closing.binding.as_ref(),
        json!({"event":"stopped","role":"api","identity":identity}),
    )
}
