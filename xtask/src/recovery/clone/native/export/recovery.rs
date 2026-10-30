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
            "summary": summary(verified)?,
            "remote_writes": 0,
        }),
    )?;
    Ok(())
}

pub(crate) fn reconcile(context: &Context) -> Result<Value> {
    let current = current_attempt(context)?;
    let (attempt, intent, verified) = verify_candidate(context, &current["sources"])?;
    let result = reconciled(context, &attempt, &intent, &verified, &current["sources"])?;
    for (_, previous) in adoptions(context)? {
        if previous != result {
            return Err("本次 export 采用结果与同一 run 的既有结果不同".into());
        }
    }
    Ok(result)
}

fn reconciled(
    context: &Context,
    attempt: &Value,
    intent: &Value,
    verified: &Value,
    sources: &Value,
) -> Result<Value> {
    let number = attempt["number"].as_u64().ok_or("原 export 序号无效")?;
    Ok(json!({
        "status": "export_reconciled",
        "origin_attempt": number,
        "origin_controller": intent["controller"],
        "origin_intent": binding(context, &context.run_dir.join(format!("export-{number:04}.intent.json")))?,
        "export": verified["binding"],
        "source_request": context.manifest["source_request"],
        "source_storage": intent["source_storage"],
        "summary": summary(verified)?,
        "origin_execution_source": attempt["sources"],
        "reconciled_execution_source": sources,
        "remote_writes": 0,
        "restore_qualified": false,
    }))
}

pub(crate) fn resume(context: &Context) -> Result<Value> {
    let current = current_attempt(context)?;
    let (adopted, receipt) = adoptions(context)?
        .into_iter()
        .rfind(|(attempt, _)| attempt["status"] == "passed")
        .ok_or("export resume 前必须先完成显式只读 reconcile")?;
    let (attempt, intent, verified) = verify_candidate(context, &current["sources"])?;
    if receipt != reconciled(context, &attempt, &intent, &verified, &adopted["sources"])? {
        return Err("export resume 的已发布 reconcile 收据与当前固定候选矛盾".into());
    }
    let result = adopted
        .get("result")
        .ok_or("reconcile attempt 缺少结果绑定")?;
    Ok(json!({
        "status": "export_verified",
        "export": verified["binding"],
        "logical_inventory_sha256": verified["export"]["logical_inventory_sha256"],
        "reconciliation": result,
        "remote_writes": 0,
        "restore_qualified": false,
    }))
}

fn candidates(context: &Context) -> Result<Vec<Value>> {
    let value = state::load(&context.run_dir, true)?;
    let mut candidates = Vec::new();
    for attempt in state::attempts(&value)? {
        if attempt["number"].as_u64().unwrap_or_default() >= context.attempt
            || attempt["stage"] != "export"
            || attempt["mode"] != "run"
        {
            continue;
        }
        let number = attempt["number"].as_u64().ok_or("原 export 序号无效")?;
        let directory = context.run_dir.join(format!("e{number:04}"));
        match std::fs::symlink_metadata(&directory) {
            Ok(metadata) if !metadata.is_dir() || digest::linked(&metadata) => {
                return Err("源导出 attempt 输出不是固定普通目录".into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            _ => {}
        }
        match std::fs::symlink_metadata(directory.join("export.json")) {
            Ok(_) => candidates.push(attempt.clone()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(candidates)
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
    let state = state::load(&context.run_dir, true)?;
    let attempt = state::attempts(&state)?
        .iter()
        .find(|attempt| attempt["number"].as_u64() == Some(number))
        .ok_or("源导出 intent 缺少原 attempt")?;
    let controller = original_controller(context, attempt)?;
    if root.get("format_version").and_then(Value::as_u64) != Some(1)
        || root.get("kind").and_then(Value::as_str) != Some("devex-clone-export-attempt")
        || root.get("attempt").and_then(Value::as_u64) != Some(number)
        || root.get("source_request") != context.manifest.get("source_request")
        || root.get("output") != Some(&json!(context.run_dir.join(format!("e{number:04}"))))
        || root.get("manifest") != Some(&state::manifest_binding(&context.run_dir)?)
        || root.get("controller") != Some(&controller)
        || root.get("execution_source") != attempt.get("sources")
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

fn current_attempt(context: &Context) -> Result<Value> {
    let value = state::load(&context.run_dir, true)?;
    let attempt = state::attempts(&value)?
        .last()
        .ok_or("缺少当前 export attempt")?;
    if attempt["number"].as_u64() != Some(context.attempt)
        || attempt["stage"] != "export"
        || attempt["mode"] != context.mode
        || attempt["status"] != "running"
    {
        return Err("源导出恢复不是当前已登记阶段".into());
    }
    Ok(attempt.clone())
}

fn original_controller(context: &Context, attempt: &Value) -> Result<Value> {
    let number = attempt["number"].as_u64().ok_or("原 export 序号无效")?;
    let mut initial = attempt.clone();
    initial["status"] = json!("running");
    initial["finished_at"] = Value::Null;
    initial["result"] = Value::Null;
    initial["error_type"] = Value::Null;
    let (binding, owner) = state::controller_record(&context.run_dir, number, &initial)?;
    super::super::lifecycle::identity::validate(&owner, &context.run_dir)?;
    if owner["manifest_sha256"] != state::manifest_binding(&context.run_dir)?["sha256"] {
        return Err("源导出控制器未绑定原运行清单".into());
    }
    Ok(binding)
}

fn product(sources: &Value) -> Result<&Value> {
    let value = sources
        .pointer("/fingerprints/product")
        .ok_or("export 恢复缺少产品源码指纹")?;
    let root = json_io::exact(value, &["sha256", "files"], "产品源码指纹")?;
    let sha = root["sha256"].as_str().ok_or("产品源码摘要无效")?;
    if sha.len() != 64
        || !sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || root["files"].as_u64().is_none()
    {
        return Err("产品源码指纹无效".into());
    }
    Ok(value)
}

fn verify_candidate(context: &Context, sources: &Value) -> Result<(Value, Value, Value)> {
    let candidates = candidates(context)?;
    if candidates.len() != 1 || candidates[0]["status"] != "failed" {
        return Err("export reconcile 必须恰有一个由原 failed attempt 推导的完整候选".into());
    }
    let attempt = &candidates[0];
    let number = attempt["number"].as_u64().ok_or("原 export 序号无效")?;
    let intent = intent(context, number)?;
    let intent_path = context
        .run_dir
        .join(format!("export-{number:04}.intent.json"));
    let intent_before = binding(context, &intent_path)?;
    if product(&intent["execution_source"])? != product(sources)? {
        return Err("原 export attempt 与当前恢复执行的产品源码指纹不同".into());
    }
    let storage = super::super::resources::storage::registered::current(
        &context.backend,
        &context.run_dir,
        crate::cli::CloneSide::Source,
    )?;
    if json!(storage) != intent["source_storage"] {
        return Err("源 storage provenance 与原 export attempt 不同".into());
    }
    let export = binding_for(context, number)?;
    let verified = verify::binding(context, &export)?;
    if verified["export"]["request"] != context.manifest["source_request"]
        || binding(context, &intent_path)? != intent_before
        || self::intent(context, number)? != intent
        || binding_for(context, number)? != export
        || super::super::resources::storage::registered::current(
            &context.backend,
            &context.run_dir,
            crate::cli::CloneSide::Source,
        )? != storage
    {
        return Err("候选 export 的请求或 storage 前后复核不一致".into());
    }
    record_candidate_verified(context, number, &verified)?;
    Ok((attempt.clone(), intent, verified))
}

fn adoptions(context: &Context) -> Result<Vec<(Value, Value)>> {
    let value = state::load(&context.run_dir, true)?;
    let mut results: Vec<(Value, Value)> = Vec::new();
    for attempt in state::attempts(&value)? {
        if attempt["number"].as_u64().unwrap_or_default() >= context.attempt
            || attempt["stage"] != "export"
            || attempt["result"].is_null()
        {
            continue;
        }
        let path = manifest::bound_file(
            &context.backend,
            &attempt["result"],
            "export 采用结果",
            true,
        )?;
        let receipt = json_io::read(&path)?;
        if receipt["status"] != "export_reconciled" {
            continue;
        }
        json_io::exact(
            &receipt,
            &[
                "status",
                "origin_attempt",
                "origin_controller",
                "origin_intent",
                "export",
                "source_request",
                "source_storage",
                "summary",
                "origin_execution_source",
                "reconciled_execution_source",
                "remote_writes",
                "restore_qualified",
            ],
            "export 采用结果",
        )?;
        if attempt["mode"] != "reconcile"
            || !matches!(attempt["status"].as_str(), Some("passed" | "failed"))
            || receipt["reconciled_execution_source"] != attempt["sources"]
            || results
                .first()
                .is_some_and(|(_, previous)| *previous != receipt)
        {
            return Err("export 采用结果不属于同一已收尾 reconcile 或存在矛盾结果".into());
        }
        results.push((attempt.clone(), receipt));
    }
    Ok(results)
}

fn summary(verified: &Value) -> Result<Value> {
    let export = verified.get("export").ok_or("已复核导出缺少 export")?;
    Ok(json!({
        "export_sha256": digest::plan_hash(export)?,
        "generation_sha256": digest::plan_hash(verified.get("generation").ok_or("已复核导出缺少 generation")?)?,
        "proof_files_sha256": digest::plan_hash(verified.get("proof_files").ok_or("已复核导出缺少 proof_files")?)?,
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
    match std::fs::symlink_metadata(&path) {
        Ok(_) if json_io::read(&path)? != value => {
            return Err("源导出完整复核摘要与既有候选矛盾".into());
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
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
