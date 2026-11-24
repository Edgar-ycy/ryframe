//! 来源运行收据与原始行证据只读取同代文件；不承担远端写入。
use super::super::super::inventory;
use super::{documents, model};
use crate::utc_timestamp::now_rfc3339;
use crate::{Result, recovery::clone::native::resources::common};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(super) const RECEIPT: &[&str] = &[
    "format_version",
    "kind",
    "status",
    "source_generation",
    "dataset_lineage",
    "before",
    "after",
    "producer",
    "verification",
    "login_audit",
    "cache_cleanup",
    "evidence",
    "write_effects",
    "started_at",
    "verified_at",
    "remote_writes",
    "restore_qualified",
];
const PRODUCER: &[(&str, &str)] = &[
    ("intent", "source-producer-intent.json"),
    ("process", "source-producer.json"),
    ("ready", "source-producer-ready.json"),
    ("stdout", "source-producer.stdout.json"),
    ("stderr", "source-producer.stderr.log"),
    ("completion", "source-producer-completion.json"),
];

pub(super) fn directory(args: &Value) -> Result<(PathBuf, PathBuf)> {
    let backend = PathBuf::from(documents::text(&args["backend"], "来源后端")?);
    let directory = PathBuf::from(documents::text(&args["directory"], "来源证据目录")?);
    common::local_directory(&backend, &directory, "来源证据目录")?;
    Ok((backend, directory))
}

pub(super) fn producer_bindings(directory: &Path) -> Result<Value> {
    let mut result = json!({});
    for (key, name) in PRODUCER {
        result[*key] = common::binding(&directory.join(name))?;
    }
    Ok(result)
}

pub(super) fn create(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "directory",
            "lineage",
            "start",
            "started_at",
            "before",
            "after",
            "effects",
            "cleanup",
            "recovery",
        ],
        "来源运行收据生成参数",
    )?;
    let (_, directory) = directory(args)?;
    let mut value = json!({"format_version":3,"kind":"restore-source-runtime","status":"source_runtime_verified",
        "source_generation":args["start"],"dataset_lineage":args["lineage"],"before":args["before"],"after":args["after"],
        "producer":producer_bindings(&directory)?,"verification":common::binding(&directory.join("source-producer.stdout.json"))?,
        "login_audit":common::binding(&directory.join("audit/login-audit.json"))?,
        "cache_cleanup":common::binding(&directory.join("cache-cleanup.json"))?,"evidence":{
            "before":inventory::manifest(&directory.join("before"))?,"after":inventory::manifest(&directory.join("after"))?,
            "audit":inventory::manifest(&directory.join("audit"))?},"write_effects":args["effects"],
        "started_at":args["started_at"],"verified_at":now_rfc3339(),
        "remote_writes":model::remote_writes(&json!({"cleanup":args["cleanup"],"recovered":!args["recovery"].is_null()}))?,
        "restore_qualified":false});
    if !args["recovery"].is_null() {
        value["recovery"] = args["recovery"].clone();
    }
    Ok(value)
}

pub(super) fn root_entries(recovered: bool) -> BTreeSet<&'static str> {
    let mut result = BTreeSet::from([
        "before",
        "after",
        "audit",
        "source-tools",
        "source-producer-intent.json",
        "source-producer.json",
        "source-producer-ready.json",
        "source-producer.stdout.json",
        "source-producer.stderr.log",
        "source-producer-completion.json",
        "source-producer-configuration.json",
        "cache-cleanup.json",
        "source-runtime.json",
    ]);
    if recovered {
        result.extend([
            "failed.json",
            "source-verification-recovery-intent.json",
            "source-verification-recovery.json",
            "recovery-audit",
        ]);
    }
    result
}

pub(super) fn schema(args: &Value) -> Result<Value> {
    documents::exact(args, &["backend", "directory", "value"], "来源运行收据参数")?;
    let (backend, directory) = directory(args)?;
    let value = &args["value"];
    let recovered = value.get("recovery").is_some();
    let mut fields = RECEIPT.to_vec();
    if recovered {
        fields.push("recovery");
    }
    documents::exact(value, &fields, "来源运行收据")?;
    let actual = crate::recovery::runtime::dataset_authority::reads::entries(&directory)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<BTreeSet<_>>>()?;
    let mut expected = root_entries(recovered)
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let attempt_entries = super::cache_attempt::entries(&backend, &directory)?;
    if attempt_entries.is_empty() {
        return Err("已封口来源缺少原缓存 attempt".into());
    }
    expected.extend(attempt_entries.into_iter().map(str::to_owned));
    documents::exact(
        &value["producer"],
        &[
            "intent",
            "process",
            "ready",
            "stdout",
            "stderr",
            "completion",
        ],
        "来源生产者绑定",
    )?;
    if directory
        .file_name()
        .is_none_or(|name| name != "verification")
        || value["format_version"].as_u64() != Some(3)
        || value["kind"] != "restore-source-runtime"
        || value["status"] != "source_runtime_verified"
        || value["restore_qualified"] != false
        || actual != expected
        || recovered && !value["recovery"].is_object()
    {
        return Err("来源运行收据版本、状态或证据集合无效".into());
    }
    if producer_bindings(&directory)? != value["producer"] {
        return Err("来源运行收据的生产者文件绑定已变化".into());
    }
    if common::binding(&directory.join("source-producer.stdout.json"))? != value["verification"] {
        return Err("来源运行收据没有绑定原生 stdout".into());
    }
    Ok(json!({"recovered":recovered}))
}

pub(super) fn login(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["backend", "directory", "descriptor"],
        "来源登录审计参数",
    )?;
    let (backend, directory) = directory(args)?;
    let path = common::bound_file(&backend, &args["descriptor"], "来源登录审计")?;
    if path != directory.join("audit/login-audit.json") {
        return Err("来源登录审计不属于同一 verification 目录".into());
    }
    let value = documents::read(&path)?;
    documents::exact(
        &value,
        &[
            "format_version",
            "kind",
            "status",
            "before_rows",
            "after_old_rows",
            "after_new_rows",
            "verified",
            "completed_at",
        ],
        "来源登录审计",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "restore-source-login-audit"
        || value["status"] != "verified"
    {
        return Err("来源登录审计类型或状态无效".into());
    }
    for (key, name) in [
        ("before_rows", "login-before.tsv"),
        ("after_old_rows", "login-after-old.tsv"),
        ("after_new_rows", "login-after-new.tsv"),
    ] {
        if value[key] != common::binding(&directory.join("audit").join(name))? {
            return Err("来源登录审计原始行证据已变化".into());
        }
    }
    if common::binding(&path)? != args["descriptor"] {
        return Err("来源登录审计在核验期间变化".into());
    }
    Ok(value)
}

/// 组合根已取得代次事实与完整前后像；在发布成功前解释全部证据与副作用。
pub(super) fn final_check(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "directory",
            "descriptor",
            "receipt",
            "before",
            "expected_before",
            "effects",
            "cleanup",
            "recovered",
        ],
        "来源运行封口参数",
    )?;
    let (backend, directory) = directory(args)?;
    let receipt = &args["receipt"];
    let cleanup = &args["cleanup"];
    let recovered = args["recovered"].as_bool().ok_or("来源恢复标志无效")?;
    super::cache_attempt::validate_cleanup(&backend, &directory, cleanup)?;
    if args["before"] != args["expected_before"] {
        return Err("来源验收前像与 start 运行像不同".into());
    }
    if receipt["write_effects"] != args["effects"]
        || documents::array(&cleanup["after"], "来源缓存后像")?.len() != 33
        || !recovered && cleanup["execution"] != "confirmed_watch_exec"
        || receipt["remote_writes"]
            != model::remote_writes(&json!({"cleanup":cleanup,"recovered":recovered}))?
    {
        return Err("来源运行收据的业务副作用与完整像不同".into());
    }
    documents::exact(
        &receipt["evidence"],
        &["before", "after", "audit"],
        "来源验收文件清单",
    )?;
    for name in ["before", "after", "audit"] {
        if receipt["evidence"][name] != json!(inventory::manifest(&directory.join(name))?) {
            return Err("来源验收证据文件集合或字节发生变化".into());
        }
    }
    let completion = documents::read(&directory.join("source-producer-completion.json"))?;
    model::time_order(
        &json!({"label":"来源运行收据时间","values":[receipt["started_at"],completion["completed_at"],cleanup["cleaned_at"],receipt["verified_at"]]}),
    )?;
    if common::binding(&directory.join("source-runtime.json"))? != args["descriptor"] {
        return Err("来源运行收据在核验期间变化".into());
    }
    Ok(Value::Null)
}
