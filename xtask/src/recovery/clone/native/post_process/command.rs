use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::super::{json as json_io, resources::common};
use crate::Result;

const KINDS: &[(&str, &str, &str)] = &[
    (
        "session",
        "post-copy",
        "xtask/src/recovery/clone/native/session.rs",
    ),
    (
        "existing",
        "post-copy",
        "xtask/src/recovery/dataset_prepare/existing_native.rs",
    ),
    (
        "identity-apply",
        "seed-runtime",
        "xtask/src/data/performance_identities/producer.rs",
    ),
    (
        "identity-verify",
        "seed-runtime",
        "xtask/src/data/performance_identities/producer.rs",
    ),
    (
        "quota-plan",
        "seed-runtime",
        "xtask/src/data/performance_identities/clone_bridge.rs",
    ),
    (
        "quota-apply",
        "seed-runtime",
        "xtask/src/data/performance_identities/clone_bridge.rs",
    ),
    (
        "quota-reconcile",
        "seed-runtime",
        "xtask/src/data/performance_identities/clone_bridge.rs",
    ),
    (
        "department-plan",
        "seed-runtime",
        "xtask/src/data/performance_identities/clone_bridge.rs",
    ),
    (
        "department-apply",
        "seed-runtime",
        "xtask/src/data/performance_identities/clone_bridge.rs",
    ),
    (
        "department-reconcile",
        "seed-runtime",
        "xtask/src/data/performance_identities/clone_bridge.rs",
    ),
    (
        "department-verify",
        "seed-runtime",
        "xtask/src/data/performance_identities/clone_bridge.rs",
    ),
];

pub(super) fn stage(kind: &str) -> Result<&'static str> {
    KINDS
        .iter()
        .find(|item| item.0 == kind)
        .map(|item| item.1)
        .ok_or_else(|| "原生生产者类型未登记".into())
}

pub(super) fn source(backend: &Path, kind: &str) -> Result<PathBuf> {
    KINDS
        .iter()
        .find(|item| item.0 == kind)
        .map(|item| backend.join(item.2))
        .ok_or_else(|| "原生生产者类型未登记".into())
}

pub(super) fn accepts(kind: &str, mode: &str) -> bool {
    match kind {
        "session" => matches!(mode, "schedules" | "reconcile"),
        "existing" => mode == "verify",
        "identity-apply" => mode == "identities-apply",
        "identity-verify" => mode == "identities-verify",
        "quota-plan" => mode == "quotas-plan",
        "quota-apply" => mode == "quotas-apply",
        "quota-reconcile" => mode == "quotas-reconcile",
        "department-plan" => mode == "departments-plan",
        "department-apply" => mode == "departments-apply",
        "department-reconcile" => mode == "departments-reconcile",
        "department-verify" => mode == "departments-verify",
        _ => false,
    }
}

pub(super) fn build(request: &Value) -> Result<Value> {
    let object = json_io::exact(
        request,
        &[
            "backend",
            "run_dir",
            "attempt",
            "kind",
            "output",
            "tool",
            "registration",
            "configuration",
        ],
        "生产者命令请求",
    )?;
    let backend = absolute(object.get("backend").unwrap(), "后端目录")?;
    let run_dir = absolute(object.get("run_dir").unwrap(), "运行目录")?;
    let output = absolute(object.get("output").unwrap(), "attempt 目录")?;
    let attempt = object
        .get("attempt")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .ok_or("生产者 attempt 无效")?;
    let kind = json_io::text(object.get("kind").unwrap(), "生产者类型")?;
    let tool = tool(object.get("tool").unwrap())?;
    let registration = object.get("registration").unwrap();
    let hidden = match kind {
        "session" => "__copy-session",
        "existing" => "__recovery-existing-clone",
        value if value.starts_with("identity-") => "__identity-producer",
        _ if stage(kind).is_ok() => "__identity-bridge",
        _ => return Err("原生生产者类型未登记".into()),
    };
    let mut argv = vec![
        tool["path"].clone(),
        json!(hidden),
        json!("--run-dir"),
        json!(run_dir),
        json!("--attempt"),
        json!(attempt.to_string()),
    ];
    if kind == "existing" {
        let configuration = absolute(object.get("configuration").unwrap(), "已有数据配置")?;
        argv.extend([
            json!("--backend-dir"),
            json!(backend),
            json!("--plan"),
            registration["reference_plan"]["path"].clone(),
            json!("--dataset"),
            registration["dataset"]["path"].clone(),
            json!("--target-binding"),
            json!(output.join("business-target.json")),
            json!("--configuration"),
            json!(configuration),
            json!("--write"),
        ]);
    } else if let Some(mode) = kind.strip_prefix("identity-") {
        identity_args(&mut argv, registration, mode, true)?;
    } else if let Some(mode) = kind.strip_prefix("quota-") {
        argv.extend([json!("--quota-mode"), json!(mode)]);
        identity_args(&mut argv, registration, mode, false)?;
    } else if let Some(mode) = kind.strip_prefix("department-") {
        argv.extend([json!("--department-mode"), json!(mode)]);
        identity_args(&mut argv, registration, mode, false)?;
    }
    Ok(json!({"argv":argv,"source":common::binding(&source(&backend,kind)?)?,"tool":tool}))
}

fn identity_args(
    argv: &mut Vec<Value>,
    value: &Value,
    mode: &str,
    include_state: bool,
) -> Result<()> {
    let plan = value
        .get("identity_plan")
        .ok_or("身份生产者缺少 identity_plan")?;
    argv.extend([json!("--identity-plan"), plan["path"].clone()]);
    if include_state {
        argv.extend([
            json!("--identity-state"),
            value
                .get("identity_state")
                .cloned()
                .ok_or("身份生产者缺少 identity_state")?,
            json!("--identity-mode"),
            json!(mode),
        ]);
    }
    argv.extend([json!("--identity-plan-sha256"), plan["sha256"].clone()]);
    Ok(())
}

fn tool(value: &Value) -> Result<Value> {
    json_io::exact(value, &["path", "sha256"], "生产者工具")?;
    let path = absolute(&value["path"], "生产者工具")?;
    let sha = json_io::text(&value["sha256"], "生产者工具摘要")?;
    if sha.len() != 64
        || !sha
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("生产者工具摘要无效".into());
    }
    Ok(json!({"path":path,"sha256":sha}))
}

fn absolute(value: &Value, label: &str) -> Result<PathBuf> {
    let path = PathBuf::from(json_io::text(value, label)?);
    if !path.is_absolute() {
        return Err(format!("{label}必须为绝对路径").into());
    }
    Ok(path)
}
