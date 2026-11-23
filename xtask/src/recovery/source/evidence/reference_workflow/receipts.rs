use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::{exact, inputs, native, path, text, unlinked};
use crate::Result;

pub(super) fn start(args: &Value) -> Result<Value> {
    exact(
        args,
        &["plan", "command", "copy_id", "root", "receipt", "bindings"],
    )?;
    let plan = &args["plan"];
    let work = path(&plan["work_dir"])?;
    super::files::work(&json!({"plan":plan}), false)?;
    let command = text(&args["command"])?;
    if !matches!(
        command,
        "dataset" | "backup" | "restore" | "copy" | "damage"
    ) {
        return Err("参考收据只允许明确的写入阶段".into());
    }
    let name = match command {
        "restore" => {
            let side = text(&plan["target_side"])?;
            if !matches!(side, "base" | "candidate") {
                return Err("恢复收据必须明确选择base或candidate".into());
            }
            format!("restore-{side}")
        }
        "copy" => {
            let id = text(&args["copy_id"])?;
            inputs::domain::identifier(id)?;
            format!("copy-{id}")
        }
        "damage" => {
            let root = path(&args["root"])?;
            unlinked(root)?;
            let id = root
                .file_name()
                .and_then(|part| part.to_str())
                .ok_or("损坏副本缺少ID")?;
            inputs::domain::identifier(id)?;
            format!("damage-{id}")
        }
        _ => command.to_owned(),
    };
    let receipt = work.join(format!("{name}.json"));
    if !args["receipt"].is_null() && path(&args["receipt"])? != receipt {
        return Err("恢复收据路径与固定阶段不一致".into());
    }
    unlinked(&receipt)?;
    let mut started = json!({"command":command,"plan_sha256":native::digest::plan_hash(plan)?,
        "started_at":Utc::now().to_rfc3339(),"status":"running"});
    if command == "restore" {
        let bindings = exact(
            &args["bindings"],
            &[
                "target_side",
                "target_plan",
                "backup_receipt",
                "record",
                "begin_receipt",
                "runtime_registration",
            ],
        )?;
        if bindings["target_side"] != plan["target_side"] {
            return Err("恢复收据目标侧不匹配".into());
        }
        for field in [
            "target_plan",
            "backup_receipt",
            "record",
            "begin_receipt",
            "runtime_registration",
        ] {
            verify_binding(&bindings[field])?;
        }
        started
            .as_object_mut()
            .ok_or("恢复收据必须为对象")?
            .extend(bindings.clone());
    } else if !args["bindings"].is_null() {
        return Err("非恢复收据不接受恢复输入绑定".into());
    }
    native::json::write_new(&receipt, &started)?;
    let digest = native::digest::file(&receipt)?;
    Ok(json!({"path":receipt,"started":started,"bytes":digest.bytes,"sha256":digest.sha256}))
}

fn verify_binding(binding: &Value) -> Result<()> {
    exact(binding, &["path", "bytes", "sha256"])?;
    let file = path(&binding["path"])?;
    unlinked(file)?;
    let digest = native::digest::file(file)?;
    if binding["bytes"] != digest.bytes || binding["sha256"] != digest.sha256 {
        return Err("恢复输入绑定在收据发布前发生变化".into());
    }
    Ok(())
}

pub(super) fn finish(args: &Value) -> Result<Value> {
    exact(args, &["receipt", "result", "failure", "resource_images"])?;
    let receipt = &args["receipt"];
    let file = check_started(receipt)?;
    finish_started(file, receipt, args)
}

pub(super) fn check_started(receipt: &Value) -> Result<&std::path::Path> {
    exact(receipt, &["path", "started", "bytes", "sha256"])?;
    let file = path(&receipt["path"])?;
    unlinked(file)?;
    let owner = native::json::read(
        &file
            .parent()
            .ok_or("收据缺少归属目录")?
            .join("reference-owner.json"),
    )?;
    if owner["format_version"].as_u64() != Some(1)
        || owner["plan_sha256"] != receipt["started"]["plan_sha256"]
    {
        return Err("运行收据不属于当前参考目录ownership".into());
    }
    let digest = native::digest::file(file)?;
    if receipt["bytes"] != digest.bytes
        || receipt["sha256"] != digest.sha256
        || native::json::read(file)? != receipt["started"]
        || receipt["started"]["status"] != "running"
    {
        return Err("运行收据发生变化或已终结，禁止覆盖和重放".into());
    }
    Ok(file)
}

fn finish_started(file: &std::path::Path, receipt: &Value, args: &Value) -> Result<Value> {
    let started = DateTime::parse_from_rfc3339(text(&receipt["started"]["started_at"])?)?;
    let completed = Utc::now();
    if completed < started {
        return Err("收据完成时间早于开始时间".into());
    }
    let failed = !args["failure"].is_null();
    if failed && !args["result"].is_null() {
        return Err("失败收据不能包含成功结果".into());
    }
    if !failed && args["result"].is_null() {
        return Err("成功收据必须包含结果".into());
    }
    let mut value = receipt["started"].clone();
    value["status"] = json!(if failed { "failed" } else { "completed" });
    value["completed_at"] = json!(completed.to_rfc3339());
    if failed {
        validate_failure(&args["failure"])?;
        value["failure"] = args["failure"].clone();
        if !args["resource_images"].is_null() {
            value["resource_images"] = args["resource_images"].clone();
        }
    } else {
        if !args["resource_images"].is_null() {
            return Err("成功收据资源像必须属于结果".into());
        }
        value["result"] = args["result"].clone();
    }
    native::json::write_replace(file, &value)?;
    Ok(value)
}

fn validate_failure(value: &Value) -> Result<()> {
    let failure = native::json::object(value, "失败诊断")?;
    if !failure.contains_key("error_type")
        || !failure.contains_key("message")
        || failure.keys().any(|key| {
            !["error_type", "message", "notes", "stdout", "stderr"].contains(&key.as_str())
        })
    {
        return Err("失败诊断字段无效".into());
    }
    text(&failure["error_type"])?;
    text(&failure["message"])?;
    for key in ["stdout", "stderr"] {
        if let Some(value) = failure.get(key) {
            text(value)?;
        }
    }
    if let Some(notes) = failure.get("notes") {
        let notes = native::json::array(notes, "失败备注")?;
        if notes.len() > 20 {
            return Err("失败备注数量超限".into());
        }
        for note in notes {
            text(note)?;
        }
    }
    Ok(())
}
