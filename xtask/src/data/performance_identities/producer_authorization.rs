//! 原生生产者统一核验 launch、进程身份、控制器和当前 run 锁。

use crate::{
    Result,
    recovery::clone::native::{digest, json as document, lifecycle::identity},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) struct Spec<'a> {
    pub run: &'a Path,
    pub attempt: u64,
    pub stage: &'a str,
    pub mode: &'a str,
    pub producer_kind: &'a str,
    pub operation: &'a str,
    pub registration: &'a Path,
    pub source: &'a Path,
    pub command: &'a [String],
    pub tool: &'a Value,
    pub owner_field: &'a str,
}

pub(crate) fn authorize(root: &Path, spec: &Spec<'_>, grant: &Value) -> Result<String> {
    let mut fields = vec!["operation", "run_dir", "attempt", "launch", "process"];
    fields.push(spec.owner_field);
    document::exact(grant, &fields, "原生生产者启动授权")?;
    let token = grant[spec.owner_field]
        .as_str()
        .filter(|value| super::run_lock::uuid(value))
        .ok_or("原生生产者 owner 无效")?;
    if grant["operation"] != spec.operation
        || grant["run_dir"].as_str() != spec.run.to_str()
        || grant["attempt"] != spec.attempt
    {
        return Err("原生生产者授权不属于当前 run 或 attempt".into());
    }
    let output = spec
        .run
        .join(spec.stage)
        .join(format!("attempt-{:04}", spec.attempt));
    let launch_path = output.join("session-launch.json");
    let process_path = output.join("session-process.json");
    if grant["launch"] != binding(&launch_path)? || grant["process"] != binding(&process_path)? {
        return Err("原生生产者授权未绑定持久化 launch 与进程收据".into());
    }
    let launch = document::read(&launch_path)?;
    let process = document::read(&process_path)?;
    validate_receipts(spec, grant, &launch, &process)?;
    validate_controller(spec, &launch)?;
    validate_tool(root, spec, &launch)?;
    if binding(&launch_path)? != grant["launch"] || binding(&process_path)? != grant["process"] {
        return Err("原生生产者授权核验期间证据变化".into());
    }
    Ok(token.to_owned())
}

fn validate_receipts(
    spec: &Spec<'_>,
    grant: &Value,
    launch: &Value,
    process: &Value,
) -> Result<()> {
    document::exact(
        launch,
        &[
            "format_version",
            "kind",
            "run_manifest",
            "registration",
            "controller",
            "attempt",
            "producer_kind",
            "command",
            "tool",
            "source",
        ],
        "原生生产者 launch",
    )?;
    document::exact(
        process,
        &[
            "format_version",
            "kind",
            "launch",
            "identity",
            spec.owner_field,
        ],
        "原生生产者进程收据",
    )?;
    if launch["format_version"] != 1
        || launch["kind"] != "devex-post-producer-launch"
        || launch["attempt"] != spec.attempt
        || launch["producer_kind"] != spec.producer_kind
        || launch["command"] != json!(spec.command)
        || launch["run_manifest"] != binding(&spec.run.join("manifest.json"))?
        || launch["registration"] != binding(spec.registration)?
        || launch["source"] != binding(spec.source)?
        || launch["tool"] != *spec.tool
        || process["format_version"] != 1
        || process["kind"] != "devex-post-producer"
        || process["launch"] != grant["launch"]
        || process[spec.owner_field] != grant[spec.owner_field]
        || process["identity"] != identity::current()?
    {
        return Err("原生生产者 launch、进程身份或 owner 不符".into());
    }
    Ok(())
}

fn validate_controller(spec: &Spec<'_>, launch: &Value) -> Result<()> {
    let state = document::read(&spec.run.join("state.json"))?;
    let attempt = state["attempts"]
        .as_array()
        .and_then(|items| items.last())
        .ok_or("原生生产者 run 缺少当前 attempt")?;
    let controller_path = spec
        .run
        .join(format!("controller-{:04}.json", spec.attempt));
    let controller = document::read(&controller_path)?;
    let owner = document::read(&spec.run.join("run.lock/owner.json"))?;
    if attempt["number"] != spec.attempt
        || attempt["stage"] != spec.stage
        || attempt["mode"] != spec.mode
        || attempt["status"] != "running"
        || controller["format_version"] != 1
        || controller["kind"] != "devex-stage-controller"
        || controller["attempt"] != spec.attempt
        || controller["attempt_sha256"] != digest::plan_hash(attempt)?
        || controller["owner"] != owner
        || controller["owner"]["directory"].as_str() != spec.run.to_str()
        || controller["owner"]["manifest_sha256"]
            != binding(&spec.run.join("manifest.json"))?["sha256"]
        || launch["controller"] != binding(&controller_path)?
    {
        return Err("原生生产者控制器不持当前 run 锁或阶段不同".into());
    }
    let pid = controller["owner"]["identity"]["pid"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or("原生生产者控制器 PID 无效")?;
    if identity::process(pid)?.as_ref() != Some(&controller["owner"]["identity"]) {
        return Err("原生生产者控制器已退出或 PID 代次变化".into());
    }
    Ok(())
}

fn validate_tool(root: &Path, spec: &Spec<'_>, launch: &Value) -> Result<()> {
    document::exact(spec.tool, &["path", "sha256"], "原生生产者工具")?;
    let executable = std::env::current_exe()?;
    let recorded = Path::new(spec.tool["path"].as_str().ok_or("原生生产者工具路径缺失")?);
    if !same_file::is_same_file(recorded, &executable)?
        || spec.tool["sha256"] != digest::file(&executable)?.sha256
        || launch["tool"] != *spec.tool
        || !spec.source.starts_with(root)
    {
        return Err("原生生产者工具身份、摘要或来源不符".into());
    }
    Ok(())
}

pub(crate) fn binding(path: &Path) -> Result<Value> {
    let path = path.components().collect::<PathBuf>();
    let digest = digest::file(&path)?;
    Ok(json!({"path":path,"bytes":digest.bytes,"sha256":digest.sha256}))
}
