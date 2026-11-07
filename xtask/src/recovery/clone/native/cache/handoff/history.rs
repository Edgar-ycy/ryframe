//! 保留原生产者历史字段与日志名；未知启动不伪造失败关闭事件。
use super::input::Frame;
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common},
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn intent(frame: &Frame, token: &str) -> Result<()> {
    event(
        &frame.backend,
        &frame.post.runtime,
        &frame.runtime,
        None,
        json!({"event":"start-intent","role":"api","token":token,
        "artifact":frame.runtime["artifacts"]["api"],"ready_url":format!("{}/readyz",
        document::text(&frame.selected["api_url"],"API地址")?.trim_end_matches('/')),
        "configuration_sha256":frame.runtime["configuration_sha256"],
        "log":format!("api-{token}.log")}),
    )
}

pub(super) fn started(frame: &Frame, token: &str, identity: &Value) -> Result<()> {
    common::validate_process_identity(identity)?;
    if identity["executable"] != frame.runtime["artifacts"]["api"]["path"] {
        return Err("API生产者历史身份不同于冻结产物".into());
    }
    event(
        &frame.backend,
        &frame.post.runtime,
        &frame.runtime,
        None,
        json!({"event":"started","role":"api","token":token,"identity":identity}),
    )
}

fn event(
    backend: &Path,
    directory: &Path,
    runtime: &Value,
    expected: Option<&Value>,
    mut event: Value,
) -> Result<()> {
    let path = directory.join("producer-history.json");
    let before = match std::fs::symlink_metadata(&path) {
        Ok(_) => Some(common::binding(&common::common_file(
            backend,
            &path,
            "API生产者历史",
        )?)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if expected.is_some_and(|expected| before.as_ref() != Some(expected)) {
        return Err("API生产者历史在完整树关闭期间变化，拒绝发布停止事件".into());
    }
    let mut history = if let Some(binding) = &before {
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
    if let Some(binding) = &before {
        common::bound_file(backend, binding, "API生产者历史写前绑定")?;
        document::write_replace(&path, &history)
    } else {
        document::write_new(&path, &history)
    }
}

pub(super) struct Closing {
    pub binding: Option<Value>,
    pub identity: Option<Value>,
}

pub(super) fn closing(backend: &Path, directory: &Path) -> Result<Closing> {
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
        crate::recovery::clone::native::seed::source_registration::lineage::runtime::collect(
            backend, directory,
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
