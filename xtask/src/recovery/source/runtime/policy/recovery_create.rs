//! 恢复只接管可解释的失败前缀；收据构造不重放生产者或推断未知写入。
use super::super::super::{cache, inventory};
use super::{documents, model, receipts};
use crate::{Result, recovery::clone::native::resources::common};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const INTENT: &str = "source-verification-recovery-intent.json";

pub(super) fn failed_entries(args: &Value) -> Result<Value> {
    documents::exact(args, &[], "来源失败目录清单参数")?;
    let mut failed = receipts::root_entries(false);
    for name in ["after", "cache-cleanup.json", "source-runtime.json"] {
        failed.remove(name);
    }
    failed.insert("failed.json");
    Ok(json!(failed))
}

pub(super) fn prefix(args: &Value) -> Result<Value> {
    documents::exact(args, &["backend", "directory"], "来源失败前缀参数")?;
    let (backend, directory) = receipts::directory(args)?;
    let mut actual = std::fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<BTreeSet<_>>>()?;
    for name in super::cache_attempt::entries(&backend, &directory)? {
        actual.remove(name);
    }
    let failed = failed_entries(&json!({}))?
        .as_array()
        .ok_or("来源失败清单无效")?
        .iter()
        .map(|value| documents::text(value, "来源失败条目").map(str::to_owned))
        .collect::<Result<BTreeSet<_>>>()?;
    let mut pending = failed.clone();
    pending.insert(INTENT.to_owned());
    let mut auditing = pending.clone();
    auditing.insert("recovery-audit".to_owned());
    if actual != failed && actual != pending && actual != auditing {
        return Err("来源验收恢复只接受未清理、未采集后像且未封口的精确失败前缀".into());
    }
    failure(&directory)?;
    // 递归核对普通文件与链接；完整登记集合继续由生产者输入核验。
    inventory::files_request(args)?;
    Ok(Value::Null)
}

pub(super) fn intent(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "directory",
            "start",
            "lineage",
            "before",
            "successor",
            "pre_manifest",
            "authorization_cache",
            "started_at",
        ],
        "来源恢复意图构造参数",
    )?;
    prefix(&json!({"backend":args["backend"],"directory":args["directory"]}))?;
    let (_, directory) = receipts::directory(args)?;
    if directory.join(INTENT).exists()
        || json!(inventory::manifest(&directory)?) != args["pre_manifest"]
    {
        return Err("来源恢复意图创建前完整前像已变化或意图已存在".into());
    }
    let cache = &args["authorization_cache"];
    cache::observation(&json!({"value":cache,"subjects":cache["subjects"]}))?;
    model::time_order(&json!({"label":"来源恢复开始","values":[args["started_at"]]}))?;
    Ok(
        json!({"format_version":1,"kind":"restore-source-verification-recovery-intent","status":"recovering",
        "source_generation":args["start"],"dataset_lineage":args["lineage"],"before":args["before"],
        "failure":common::binding(&directory.join("failed.json"))?,"producer":receipts::producer_bindings(&directory)?,
        "login_audit":common::binding(&directory.join("audit/login-audit.json"))?,"pre_recovery_manifest":args["pre_manifest"],
        "successor":args["successor"],"authorization_cache":cache,"started_at":args["started_at"]}),
    )
}

pub(super) fn receipt(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["backend", "directory", "cleanup", "after", "completed_at"],
        "来源恢复收据构造参数",
    )?;
    let (backend, directory) = receipts::directory(args)?;
    let intent_descriptor = common::binding(&directory.join(INTENT))?;
    let intent = common::read_bound(&backend, &intent_descriptor, "来源恢复意图")?;
    let cleanup = &args["cleanup"];
    cache::receipt(&json!({"value":cleanup,"subjects":intent["authorization_cache"]["subjects"]}))?;
    super::cache_attempt::validate_cleanup(&backend, &directory, cleanup)?;
    if common::read_bound(
        &backend,
        &common::binding(&directory.join("cache-cleanup.json"))?,
        "来源缓存清理收据",
    )? != *cleanup
    {
        return Err("来源恢复缓存收据与当前文件不同".into());
    }
    model::time_order(
        &json!({"label":"来源恢复封口","values":[intent["started_at"],cleanup["cleaned_at"],args["completed_at"]]}),
    )?;
    Ok(
        json!({"format_version":1,"kind":"restore-source-verification-recovery","status":"recovered",
        "intent":intent_descriptor,"cache_cleanup":common::binding(&directory.join("cache-cleanup.json"))?,
        "after":args["after"],"historical_cleanup":super::cache_attempt::historical(cleanup)?,
        "confirmed_cleanup":super::cache_attempt::confirmed(cleanup)?,
        "login_recheck":inventory::manifest(&directory.join("recovery-audit"))?,"completed_at":args["completed_at"]}),
    )
}

pub(super) fn published(args: &Value) -> Result<Value> {
    documents::exact(args, &["expected", "actual"], "来源运行发布复核参数")?;
    if args["expected"] != args["actual"] {
        return Err("来源运行收据写入后复核不同".into());
    }
    Ok(Value::Null)
}

pub(super) fn failure(directory: &std::path::Path) -> Result<()> {
    let value = documents::read(&directory.join("failed.json"))?;
    documents::exact(&value, &["status", "error"], "来源验收原失败记录")?;
    if value["status"] != "failed"
        || !matches!(
            value["error"].as_str(),
            Some(
                "ValueError"
                    | "SourceRuntimeError"
                    | "NativeS3InvocationError"
                    | "OSError"
                    | "PermissionError"
                    | "TimeoutError"
                    | "TimeoutExpired"
                    | "FileExistsError"
            )
        )
    {
        return Err("来源失败分类未知；保留原失败文件与完整前后像，不重放".into());
    }
    Ok(())
}
