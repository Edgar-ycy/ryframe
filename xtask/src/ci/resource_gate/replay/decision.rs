use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use serde_json::{Map, Value};

use super::{
    manifest::ReplayCase,
    report::{CommandResult, ResourceGateDecision},
};

const DECISION_FORMAT_VERSION: u64 = 1;

pub(crate) fn load_decision(path: &Path) -> Result<ResourceGateDecision, String> {
    let source = fs::read_to_string(path).map_err(|error| {
        format!(
            "无法读取 resource gate decision {}：{error}",
            path.display()
        )
    })?;
    let value = serde_json::from_str::<Value>(&source)
        .map_err(|error| format!("无法读取 resource gate decision：{error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "resource gate decision 必须是 JSON 对象".to_owned())?;
    require_exact_keys(
        object,
        &["formatVersion", "recognized", "mode", "fallback", "steps"],
        "decision",
    )?;
    if object.get("formatVersion").and_then(Value::as_u64) != Some(DECISION_FORMAT_VERSION) {
        return Err(format!(
            "resource gate decision formatVersion 必须为 {DECISION_FORMAT_VERSION}"
        ));
    }
    let recognized = object
        .get("recognized")
        .and_then(Value::as_bool)
        .ok_or_else(|| "resource gate decision.recognized 必须是布尔值".to_owned())?;
    let mode = object
        .get("mode")
        .and_then(Value::as_str)
        .filter(|mode| matches!(*mode, "targeted" | "full"))
        .ok_or_else(|| "resource gate decision.mode 非法".to_owned())?;
    let fallback = match object.get("fallback") {
        Some(Value::Null) => None,
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        _ => return Err("resource gate decision.fallback 必须为空或非空字符串".to_owned()),
    };
    let steps = object
        .get("steps")
        .and_then(Value::as_array)
        .filter(|steps| !steps.is_empty())
        .ok_or_else(|| "resource gate decision.steps 必须是非空字符串数组".to_owned())?
        .iter()
        .map(|step| {
            step.as_str()
                .filter(|step| !step.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| "resource gate decision.steps 必须是非空字符串数组".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if mode == "targeted" && fallback.is_some() {
        return Err("targeted decision 不得包含 fallback".to_owned());
    }
    if mode == "full" && fallback.is_none() {
        return Err("full decision 必须包含 fallback".to_owned());
    }
    Ok(ResourceGateDecision {
        recognized,
        mode: mode.to_owned(),
        fallback,
        steps,
    })
}

fn require_exact_keys(
    object: &Map<String, Value>,
    expected: &[&str],
    label: &str,
) -> Result<(), String> {
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "{label} 字段不匹配，缺少={:?}，多余={:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        ));
    }
    Ok(())
}

pub(crate) fn load_decision_after_command(
    decision_path: &Path,
    command_log: &Path,
    return_code: i32,
) -> Result<ResourceGateDecision, String> {
    load_decision(decision_path).map_err(|error| {
        let tail = bounded_log_tail(command_log).unwrap_or_default();
        let detail = if tail.is_empty() {
            String::new()
        } else {
            format!("\n命令日志末尾：\n{tail}")
        };
        format!("{error}；命令退出码={return_code}{detail}")
    })
}

pub(crate) fn require_arm(
    case: &ReplayCase,
    result: &CommandResult,
    targeted: bool,
    require_pass: bool,
) -> Result<(), String> {
    if targeted {
        if !result.decision.recognized {
            return Err(format!(
                "replay {} 的 targeted arm 未识别激活标记",
                case.name
            ));
        }
        if result.decision.mode != case.targeted_mode {
            return Err(format!(
                "replay {} 的 targeted mode 应为 {}，实际为 {}",
                case.name, case.targeted_mode, result.decision.mode
            ));
        }
    } else if result.decision.recognized || result.decision.mode != "full" {
        return Err(format!(
            "replay {} 的 full arm 未执行未激活的完整门禁",
            case.name
        ));
    }
    if require_pass && !result.passed {
        let detail = result
            .failure_tail
            .as_ref()
            .map(|tail| format!("\n失败日志末尾：\n{tail}"))
            .unwrap_or_default();
        return Err(format!("replay prime 案例失败：{}{detail}", case.name));
    }
    Ok(())
}

pub(crate) fn bounded_log_tail(path: &Path) -> Result<String, String> {
    const LIMIT: u64 = 8192;
    let mut file = File::open(path)
        .map_err(|error| format!("无法读取 replay 命令日志 {}：{error}", path.display()))?;
    let length = file
        .metadata()
        .map_err(|error| format!("无法读取 replay 命令日志大小：{error}"))?
        .len();
    file.seek(SeekFrom::Start(length.saturating_sub(LIMIT)))
        .map_err(|error| format!("无法定位 replay 命令日志：{error}"))?;
    let mut body = Vec::new();
    file.read_to_end(&mut body)
        .map_err(|error| format!("无法读取 replay 命令日志末尾：{error}"))?;
    Ok(String::from_utf8_lossy(&body)
        .replace('\0', "")
        .trim()
        .to_owned())
}
