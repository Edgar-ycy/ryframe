//! 同代失败前缀与未知缓存写入恢复；不重放生产者或删除远端资源。
use super::super::super::{cache, inventory};
use super::{documents, model, receipts};
use crate::{Result, recovery::clone::native::resources::common};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

const INTENT: &str = "source-verification-recovery-intent.json";
const RECEIPT: &str = "source-verification-recovery.json";
const AUDIT: &str = "recovery-audit";
pub(super) const INTENT_FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "status",
    "source_generation",
    "dataset_lineage",
    "failure",
    "producer",
    "login_audit",
    "before",
    "pre_recovery_manifest",
    "successor",
    "authorization_cache",
    "started_at",
];
const RECEIPT_FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "status",
    "intent",
    "cache_cleanup",
    "after",
    "historical_cleanup",
    "confirmed_cleanup",
    "login_recheck",
    "completed_at",
];

fn intent_identity(value: &Value, args: &Value, directory: &Path) -> Result<()> {
    documents::exact(value, INTENT_FIELDS, "来源验收恢复 intent")?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "restore-source-verification-recovery-intent"
        || value["status"] != "recovering"
        || value["source_generation"] != args["start"]
        || value["dataset_lineage"] != args["lineage"]
        || value["failure"] != common::binding(&directory.join("failed.json"))?
        || value["producer"] != receipts::producer_bindings(directory)?
        || value["login_audit"] != args["login_audit"]
        || value["before"] != args["before"]
    {
        return Err("来源验收恢复 intent 没有绑定原失败前缀或当前 successor".into());
    }
    Ok(())
}

fn unchanged(path: &Path, expected: &Value) -> Result<()> {
    if common::binding(path)? != *expected {
        return Err("来源验收恢复 intent 在核验期间变化".into());
    }
    Ok(())
}

pub(super) fn pending(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "directory",
            "start",
            "lineage",
            "before",
            "successor",
            "subjects",
        ],
        "来源待续作恢复参数",
    )?;
    let (backend, directory) = receipts::directory(args)?;
    let path = directory.join(INTENT);
    let descriptor = common::binding(&path)?;
    let value = common::read_bound(&backend, &descriptor, "待续作来源验收恢复 intent")?;
    let mut identity_args = args.clone();
    identity_args["login_audit"] = common::binding(&directory.join("audit/login-audit.json"))?;
    intent_identity(&value, &identity_args, &directory)?;
    if value["successor"] != args["successor"] {
        return Err("待续作来源验收恢复 intent 与失败前缀或当前 successor 不同".into());
    }
    let current = original_manifest(&backend, &directory, &value["pre_recovery_manifest"], false)?;
    if json!(current) != value["pre_recovery_manifest"] {
        return Err("待续作来源验收恢复 intent 绑定的原完整 manifest 已变化".into());
    }
    cache::observation(&json!({"value":value["authorization_cache"],"subjects":args["subjects"]}))?;
    model::time_order(&json!({"label":"来源恢复开始时间","values":[value["started_at"]]}))?;
    unchanged(&path, &descriptor)?;
    Ok(value)
}

pub(super) fn chain(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["backend", "directory", "descriptor", "receipt", "cleanup"],
        "来源恢复证据链参数",
    )?;
    let (backend, directory) = receipts::directory(args)?;
    let path = common::bound_file(&backend, &args["descriptor"], "来源验收恢复收据")?;
    if path != directory.join(RECEIPT) {
        return Err("来源验收恢复收据不属于同一 verification 目录".into());
    }
    let recovery = documents::read(&path)?;
    documents::exact(&recovery, RECEIPT_FIELDS, "来源验收恢复收据")?;
    let intent_path = common::bound_file(&backend, &recovery["intent"], "来源恢复 intent")?;
    if intent_path != directory.join(INTENT) {
        return Err("来源验收恢复 intent 不属于同一 verification 目录".into());
    }
    let intent = documents::read(&intent_path)?;
    let receipt = &args["receipt"];
    let cleanup = &args["cleanup"];
    intent_identity(
        &intent,
        &json!({"start":receipt["source_generation"],"lineage":receipt["dataset_lineage"],
        "login_audit":receipt["login_audit"],"before":receipt["before"]}),
        &directory,
    )?;
    super::recovery_create::failure(&directory)?;
    if intent["successor"]["snapshot"]["clean"] != true {
        return Err("来源验收恢复工具不是已提交的干净 successor".into());
    }
    let observation = &intent["authorization_cache"];
    cache::observation(&json!({"value":observation,"subjects":observation["subjects"]}))?;
    if observation["namespace"] != cleanup["namespace"]
        || observation["before"] != cleanup["before"]
    {
        return Err("来源验收恢复缓存清理没有绑定 intent 的精确前像".into());
    }
    let pre_manifest =
        documents::array(&intent["pre_recovery_manifest"], "来源恢复前完整 manifest")?;
    let original = original_manifest(&backend, &directory, &intent["pre_recovery_manifest"], true)?;
    let top_level = pre_manifest
        .iter()
        .map(|row| {
            documents::text(&row["path"], "来源恢复清单路径")
                .map(|path| path.split('/').next().unwrap_or_default())
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let mut failed = receipts::root_entries(false);
    for name in ["after", "cache-cleanup.json", "source-runtime.json"] {
        failed.remove(name);
    }
    failed.insert("failed.json");
    for name in [
        super::cache_attempt::ATTEMPT,
        super::cache_attempt::EXECUTION,
    ] {
        if top_level.contains(name) {
            failed.insert(name);
        }
    }
    if pre_manifest != original || top_level != failed {
        return Err("来源验收恢复前完整 manifest 或原文件发生变化".into());
    }
    super::cache_attempt::validate_cleanup(&backend, &directory, cleanup)?;
    let confirmed = super::cache_attempt::confirmed(cleanup)?;
    if recovery["format_version"].as_u64() != Some(1)
        || recovery["kind"] != "restore-source-verification-recovery"
        || recovery["status"] != "recovered"
        || recovery["cache_cleanup"] != receipt["cache_cleanup"]
        || recovery["after"] != receipt["after"]
        || recovery["historical_cleanup"] != super::cache_attempt::historical(cleanup)?
        || recovery["confirmed_cleanup"] != confirmed
    {
        return Err("来源验收恢复收据没有绑定确认的恢复清理".into());
    }
    if json!(inventory::manifest(&directory.join(AUDIT))?) != recovery["login_recheck"] {
        return Err("来源验收恢复登录日志核对证据文件集合或字节发生变化".into());
    }
    model::time_order(
        &json!({"label":"来源恢复时间","values":[intent["started_at"],cleanup["cleaned_at"],recovery["completed_at"]]}),
    )?;
    unchanged(&intent_path, &recovery["intent"])?;
    unchanged(&path, &args["descriptor"])?;
    Ok(json!({"recovery":recovery,"successor":intent["successor"]}))
}

pub(super) fn cache_publication(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "directory",
            "output",
            "observation",
            "after",
            "cleaned_at",
        ],
        "来源缓存结果参数",
    )?;
    let state = super::cache_attempt::Frame::open(args)?.state()?;
    let observation = &args["observation"];
    if state["observation"] != *observation {
        return Err("缓存清理发布没有绑定唯一 attempt 的原前像".into());
    }
    let execution = match state["status"].as_str() {
        Some("confirmed") => "confirmed_watch_exec",
        Some("unknown_exec") => "reconciled_unknown_exec_result",
        _ => return Err("没有执行或明确未执行 UNLINK，不能发布清理成功；保留现场人工核对".into()),
    };
    let expired = documents::array(&observation["before"], "缓存完整前像")?
        .iter()
        .filter(|row| row["snapshot"]["type"] == "none")
        .map(|row| row["snapshot"]["key"].clone())
        .collect::<Vec<_>>();
    let value = json!({"format_version":2,"kind":"restore-source-authorization-cache-cleanup","status":"cleaned",
        "namespace":observation["namespace"],"before":observation["before"],"deleted":state["result"]["deleted"],
        "expired_snapshot_keys":expired,"after":args["after"],"execution":execution,"cleaned_at":args["cleaned_at"],
        "attempt":state["attempt"],"execution_record":state["execution"],"attempt_phase":state["attempt_phase"]});
    let receipt = cache::receipt(&json!({"value":value,"subjects":observation["subjects"]}))?;
    let (backend, directory) = receipts::directory(args)?;
    super::cache_attempt::validate_cleanup(&backend, &directory, &receipt)?;
    Ok(receipt)
}

pub(super) fn facts(args: &Value) -> Result<Value> {
    documents::exact(args, &["backend", "receipt"], "来源运行事实选择参数")?;
    let receipt = &args["receipt"];
    let backend = Path::new(documents::text(&args["backend"], "来源后端")?);
    let recovered = receipt.get("recovery").is_some();
    let mut fields = receipts::RECEIPT.to_vec();
    if recovered {
        fields.push("recovery");
    }
    documents::exact(receipt, &fields, "来源运行事实收据")?;
    if !recovered {
        return Ok(json!({"recovered":false,"successor":null}));
    }
    if !receipt["recovery"].is_object() {
        return Err("来源运行收据不能选择唯一普通或恢复事实 validator".into());
    }
    let recovery_path = common::bound_file(backend, &receipt["recovery"], "来源恢复收据")?;
    let recovery = documents::read(&recovery_path)?;
    documents::exact(&recovery, RECEIPT_FIELDS, "来源验收恢复收据")?;
    let intent_path = common::bound_file(backend, &recovery["intent"], "来源恢复 intent")?;
    let intent = documents::read(&intent_path)?;
    documents::exact(&intent, INTENT_FIELDS, "来源验收恢复 intent")?;
    if recovery_path.file_name().is_none_or(|name| name != RECEIPT)
        || intent_path
            != recovery_path
                .parent()
                .ok_or("来源恢复收据缺少父目录")?
                .join(INTENT)
        || intent["source_generation"] != receipt["source_generation"]
    {
        return Err("来源运行恢复事实没有绑定同代 intent".into());
    }
    Ok(json!({"recovered":true,"successor":intent["successor"]}))
}

/// 已存在的 attempt/result 是前像的一部分；仅真正首次恢复可新增这一对。
fn original_manifest(
    backend: &Path,
    directory: &Path,
    baseline: &Value,
    completed: bool,
) -> Result<Vec<Value>> {
    let before = documents::array(baseline, "恢复前完整清单")?;
    let has_attempt = before
        .iter()
        .any(|row| row["path"] == super::cache_attempt::ATTEMPT);
    let has_execution = before
        .iter()
        .any(|row| row["path"] == super::cache_attempt::EXECUTION);
    if has_execution && !has_attempt {
        return Err("恢复前像中 execution 缺少原 attempt".into());
    }
    super::cache_attempt::entries(backend, directory)?;
    let mut result = Vec::new();
    for row in inventory::manifest(directory)? {
        let path = documents::text(&row["path"], "恢复清单路径")?;
        let cache = matches!(
            path,
            super::cache_attempt::ATTEMPT | super::cache_attempt::EXECUTION
        );
        if path == INTENT
            || path.starts_with(&format!("{AUDIT}/"))
            || completed
                && (path == RECEIPT
                    || path == "cache-cleanup.json"
                    || path == "source-runtime.json"
                    || path.starts_with("after/"))
            || !has_attempt && cache
        {
            continue;
        }
        result.push(row);
    }
    Ok(result)
}
