use std::path::Path;

use serde_json::{Value, json};

use super::super::{
    digest, json as json_io,
    lifecycle::{manifest, state},
};
use super::{input::Context, verify};
use crate::Result;

pub(crate) fn reject_reexport(backend: &Path, run_dir: &Path, before: u64) -> Result<()> {
    if before == 0 {
        return Ok(());
    }
    let value = state::load(run_dir, true)?;
    for attempt in state::attempts(&value)? {
        if attempt["number"].as_u64().unwrap_or_default() >= before
            || attempt["stage"].as_str() != Some("export")
        {
            continue;
        }
        let number = attempt["number"].as_u64().unwrap_or_default();
        if run_dir
            .join(format!("e{number:04}"))
            .join("export.json")
            .exists()
        {
            return Err(
                "既有 export attempt 已写出完整候选；必须先显式 reconcile，禁止重新完整导出".into(),
            );
        }
    }
    let _ = backend;
    Ok(())
}

pub(crate) fn prepare_intent(context: &Context) -> Result<()> {
    let value = state::load(&context.run_dir, true)?;
    let attempt = state::attempts(&value)?
        .iter()
        .find(|attempt| attempt["number"].as_u64() == Some(context.attempt))
        .ok_or("找不到当前源导出 attempt")?
        .clone();
    if attempt["stage"].as_str() != Some("export")
        || attempt["mode"].as_str() != Some(context.mode.as_str())
        || attempt["status"].as_str() != Some("running")
    {
        return Err("源导出 intent 与当前阶段记录不同".into());
    }
    let (controller, _) = state::controller_record(&context.run_dir, context.attempt, &attempt)?;
    let source_storage = super::super::resources::storage::registered::current(
        &context.backend,
        &context.run_dir,
        crate::cli::CloneSide::Source,
    )?;
    let output = context.output();
    if output.exists() {
        return Err("源导出 attempt 输出目录已存在，拒绝覆盖".into());
    }
    json_io::write_new(
        &context
            .run_dir
            .join(format!("export-{:04}.intent.json", context.attempt)),
        &json!({
            "format_version": 1,
            "kind": "devex-clone-export-attempt",
            "attempt": context.attempt,
            "output": output,
            "manifest": state::manifest_binding(&context.run_dir)?,
            "controller": controller,
            "source_request": context.manifest["source_request"],
            "source_storage": source_storage,
            "execution_source": attempt["sources"],
            "remote_operations": "read_only",
        }),
    )?;
    Ok(())
}

pub(crate) fn record_verified(context: &Context, verified: &Value) -> Result<()> {
    let intent = intent(context, context.attempt)?;
    let path = context
        .run_dir
        .join(format!("export-{:04}.verified.json", context.attempt));
    json_io::write_new(
        &path,
        &json!({
            "format_version": 1,
            "kind": "devex-clone-export-verified",
            "attempt": context.attempt,
            "intent": intent_binding(context)?,
            "export": verified["binding"],
            "source_storage": intent["source_storage"],
            "summary": verified["summary"],
            "remote_writes": 0,
        }),
    )?;
    Ok(())
}

pub(crate) fn reconcile(context: &Context) -> Result<Value> {
    let attempt = candidates(context, "export")?
        .into_iter()
        .next()
        .ok_or("export reconcile 必须恰有一个由原 failed attempt 推导的完整候选")?;
    if candidates(context, "export")?.len() != 1 || attempt["status"].as_str() != Some("failed") {
        return Err("export reconcile 必须恰有一个由原 failed attempt 推导的完整候选".into());
    }
    let number = attempt["number"].as_u64().unwrap_or_default();
    let intent = intent(context, number)?;
    let export = binding_for(context, number)?;
    let verified = verify::binding(context, &export)?;
    let summary = summary(&verified)?;
    record_candidate_verified(context, number, &verified)?;
    if verified["export"]["request"] != context.manifest["source_request"] {
        return Err("候选 export 的请求与固定运行清单不同".into());
    }
    Ok(json!({
        "status": "export_reconciled",
        "origin_attempt": number,
        "origin_controller": intent["controller"],
        "origin_intent": binding(context, &context.run_dir.join(format!("export-{number:04}.intent.json")))?,
        "export": export,
        "source_request": context.manifest["source_request"],
        "source_storage": intent["source_storage"],
        "summary": summary,
        "origin_execution_source": attempt["sources"],
        "reconciled_execution_source": attempt["sources"],
        "remote_writes": 0,
        "restore_qualified": false,
    }))
}

pub(crate) fn resume(context: &Context) -> Result<Value> {
    let adopted = state::attempts(&state::load(&context.run_dir, true)?)?
        .iter()
        .rfind(|attempt| {
            attempt["number"].as_u64().unwrap_or_default() < context.attempt
                && attempt["stage"].as_str() == Some("export")
                && attempt["mode"].as_str() == Some("reconcile")
                && attempt["status"].as_str() == Some("passed")
        })
        .cloned()
        .ok_or("export resume 前必须先完成显式只读 reconcile")?;
    let result = adopted
        .get("result")
        .ok_or("reconcile attempt 缺少结果绑定")?;
    let path = manifest::bound_file(&context.backend, result, "reconcile 结果", true)?;
    let receipt = json_io::read(&path)?;
    let verified = verify::binding(
        context,
        receipt.get("export").ok_or("reconcile 收据缺少 export")?,
    )?;
    Ok(json!({
        "status": "export_verified",
        "export": verified["binding"],
        "logical_inventory_sha256": verified["export"]["logical_inventory_sha256"],
        "reconciliation": result,
        "remote_writes": 0,
        "restore_qualified": false,
    }))
}

fn candidates(context: &Context, mode: &str) -> Result<Vec<Value>> {
    let value = state::load(&context.run_dir, true)?;
    Ok(state::attempts(&value)?
        .iter()
        .filter(|attempt| {
            attempt["number"].as_u64().unwrap_or_default() < context.attempt
                && attempt["stage"].as_str() == Some("export")
                && attempt["mode"].as_str() == Some(mode)
                && attempt["status"].as_str() == Some("failed")
        })
        .filter(|attempt| {
            let number = attempt["number"].as_u64().unwrap_or_default();
            context
                .run_dir
                .join(format!("e{number:04}/export.json"))
                .exists()
        })
        .cloned()
        .collect())
}

fn intent(context: &Context, number: u64) -> Result<Value> {
    let path = context
        .run_dir
        .join(format!("export-{number:04}.intent.json"));
    let value = json_io::read(&path)?;
    let root = json_io::exact(
        &value,
        &[
            "format_version",
            "kind",
            "attempt",
            "output",
            "manifest",
            "controller",
            "source_request",
            "source_storage",
            "execution_source",
            "remote_operations",
        ],
        "源导出 intent",
    )?;
    if root.get("format_version").and_then(Value::as_u64) != Some(1)
        || root.get("kind").and_then(Value::as_str) != Some("devex-clone-export-attempt")
        || root.get("attempt").and_then(Value::as_u64) != Some(number)
        || root.get("source_request") != context.manifest.get("source_request")
        || root.get("remote_operations").and_then(Value::as_str) != Some("read_only")
    {
        return Err("源导出 intent 与固定 manifest 或原执行源码不符".into());
    }
    Ok(value)
}

fn binding_for(context: &Context, number: u64) -> Result<Value> {
    let path = context.run_dir.join(format!("e{number:04}/export.json"));
    let digest = digest::file(&path)?;
    manifest::binding(&path, &digest)
}

fn summary(verified: &Value) -> Result<Value> {
    let export = verified.get("export").ok_or("已复核导出缺少 export")?;
    Ok(json!({
        "export_sha256": digest::plan_hash(export)?,
        "logical_inventory_sha256": export["logical_inventory_sha256"],
        "databases": export["databases"].as_array().map(Vec::len).unwrap_or(0),
        "objects": export["objects"].as_array().map(|items| items.iter().map(|bucket| bucket["entries"].as_array().map(Vec::len).unwrap_or(0)).sum::<usize>()).unwrap_or(0),
    }))
}

fn record_candidate_verified(context: &Context, number: u64, verified: &Value) -> Result<()> {
    let intent_path = context
        .run_dir
        .join(format!("export-{number:04}.intent.json"));
    let intent = intent(context, number)?;
    let value = json!({
        "format_version": 1,
        "kind": "devex-clone-export-verified",
        "attempt": number,
        "intent": manifest::binding(&intent_path, &digest::file(&intent_path)?)?,
        "export": verified["binding"],
        "source_storage": intent["source_storage"],
        "summary": summary(verified)?,
        "remote_writes": 0,
    });
    let path = context
        .run_dir
        .join(format!("export-{number:04}.verified.json"));
    if path.exists() {
        if json_io::read(&path)? != value {
            return Err("源导出完整复核摘要与既有候选矛盾".into());
        }
        return Ok(());
    }
    json_io::write_new(&path, &value)?;
    Ok(())
}

fn binding(_context: &Context, path: &Path) -> Result<Value> {
    let digest = digest::file(path)?;
    manifest::binding(path, &digest)
}

fn intent_binding(context: &Context) -> Result<Value> {
    let path = context
        .run_dir
        .join(format!("export-{:04}.intent.json", context.attempt));
    manifest::binding(&path, &digest::file(&path)?)
}
