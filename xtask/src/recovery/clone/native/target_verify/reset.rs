use std::{collections::BTreeSet, path::Path};

use regex::Regex;
use serde_json::{Value, json};

use super::super::{digest, json as json_io};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const PHASES: [&str; 8] = [
    "preflight",
    "object_storage",
    "redis",
    "databases",
    "control_baseline",
    "tenant_baselines",
    "verification",
    "release",
];
const IDENTITY: [&str; 5] = [
    "environment",
    "scope_id",
    "code_sha",
    "config_sha",
    "credential_version",
];

// 复核最终 release 及其直接前像，不把同一计划的 reused 报告当成本次完成。
pub(crate) fn completed(
    backend: &Path,
    target: &Path,
    manifest: &Value,
    sha: &str,
) -> Result<Value> {
    validate_local_test_path(target, backend, LocalTestPathKind::ExistingDirectory)?;
    let scope = json_io::text(&manifest["scope_id"], "reset scope")?;
    super::super::input::name(scope)?;
    if !Regex::new(r"^[a-f0-9]{64}$")?.is_match(sha) {
        return Err("reset 计划摘要无效".into());
    }
    let stem = format!("test-{scope}-{sha}");
    let directory = target.join("reset-state");
    let report_path = directory.join(format!("{stem}.report.json"));
    let ledger_path = directory.join(format!("{stem}.ledger.json"));
    let previous_path = directory.join(format!(".{stem}.ledger.json.previous"));
    let report = read(backend, &report_path)?;
    let ledger = read(backend, &ledger_path)?;
    let previous = read(backend, &previous_path)?;
    validate_terminal(manifest, sha, &report, &ledger)?;
    validate_previous(&previous, &ledger)?;
    validate_resources(manifest, &report)?;
    Ok(json!({
        "report": descriptor(target, &report_path)?,
        "ledger": descriptor(target, &ledger_path)?,
        "completed_at": report["completed_at"],
        "status": "completed",
    }))
}

fn read(backend: &Path, path: &Path) -> Result<Value> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)?;
    json_io::read(path)
}

fn descriptor(root: &Path, path: &Path) -> Result<Value> {
    let relative = path.strip_prefix(root)?;
    let actual = digest::file(path)?;
    Ok(json!({
        "file": relative.to_str().ok_or("reset 证据相对路径必须为 UTF-8")?,
        "bytes": actual.bytes,
        "sha256": actual.sha256,
    }))
}

fn validate_terminal(manifest: &Value, sha: &str, report: &Value, ledger: &Value) -> Result<()> {
    for value in [report, ledger] {
        if value["plan_hash"].as_str() != Some(sha)
            || IDENTITY
                .iter()
                .any(|key| manifest.get(*key).is_none() || value.get(*key) != manifest.get(*key))
        {
            return Err("reset 报告或账本不属于当前精确 plan".into());
        }
        let phases = json_io::exact(&value["phases"], &PHASES, "reset phases")?;
        if phases.values().any(|phase| {
            phase["status"] != "complete"
                || phase["completed_at"].as_str().is_none_or(str::is_empty)
        }) {
            return Err("reset 有未完成阶段或锁释放证据缺失".into());
        }
        let resources = json_io::object(&value["resources"], "reset resources")?;
        if resources.is_empty()
            || resources
                .values()
                .any(|resource| resource["status"] != "complete")
        {
            return Err("reset 存在未确认资源".into());
        }
    }
    if report["report_version"].as_u64() != Some(2)
        || ledger["ledger_version"].as_u64() != Some(4)
        || report["status"] != "completed"
        || !report.get("failed_phase").is_some_and(Value::is_null)
        || report["completed_at"] != ledger["phases"]["release"]["completed_at"]
        || report["phases"] != ledger["phases"]
        || report["resources"] != ledger["resources"]
    {
        return Err("fresh 初始化必须本次 completed；reused、失败或释放中断均拒绝".into());
    }
    Ok(())
}

fn validate_previous(previous: &Value, ledger: &Value) -> Result<()> {
    let before = json_io::object(previous, "reset 上一版账本")?;
    let current = json_io::object(ledger, "reset 最终账本")?;
    if !before.keys().eq(current.keys())
        || current.iter().any(|(key, value)| {
            !matches!(key.as_str(), "updated_at" | "phases") && before.get(key) != Some(value)
        })
        || previous["updated_at"].as_str().is_none()
        || ledger["updated_at"].as_str().is_none()
        || previous["updated_at"].as_str() > ledger["updated_at"].as_str()
    {
        return Err("reset 上一版账本不属于最终账本的直接前像".into());
    }
    let phases = json_io::exact(&previous["phases"], &PHASES, "reset 前像 phases")?;
    if PHASES
        .iter()
        .filter(|key| **key != "release")
        .any(|key| phases.get(*key) != ledger["phases"].get(*key))
    {
        return Err("reset 上一版账本含有非 release 阶段变化".into());
    }
    let mut expected = ledger["phases"]["release"].clone();
    let release = expected
        .as_object_mut()
        .ok_or("reset release 阶段必须为对象")?;
    release.insert("status".into(), json!("running"));
    release.insert("completed_at".into(), Value::Null);
    if previous["phases"]["release"] != expected {
        return Err("reset 上一版账本没有证明最终 release 的直接前像".into());
    }
    Ok(())
}

fn validate_resources(manifest: &Value, report: &Value) -> Result<()> {
    let databases = json_io::array(&manifest["databases"], "reset plan databases")?;
    let expected = databases
        .iter()
        .map(|database| {
            let host = json_io::text(&database["host"], "reset MySQL host")?;
            let port = database["port"]
                .as_u64()
                .filter(|port| *port > 0 && *port <= u16::MAX.into())
                .ok_or("reset MySQL port 无效")?;
            let name = json_io::text(&database["database"], "reset MySQL database")?;
            Ok(format!("{host}:{port}/{name}"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    if expected.len() != databases.len()
        || strings(&report["databases"], "reset 完成数据库")? != expected
    {
        return Err("reset 完成报告数据库集合不符".into());
    }
    let prefixes = json_io::array(
        &manifest["object_storage"]["prefixes"],
        "reset plan prefixes",
    )?;
    let expected = prefixes
        .iter()
        .map(|item| {
            Ok(format!(
                "{}:{}",
                json_io::text(&item["bucket"], "reset bucket")?,
                json_io::text(&item["prefix"], "reset prefix")?
            ))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    if report["redis_namespace"] != manifest["redis"]["namespace"]
        || strings(&report["object_prefixes"], "reset 完成对象前缀")? != expected
    {
        return Err("reset 完成报告 Redis 或对象集合不符".into());
    }
    Ok(())
}

fn strings(value: &Value, label: &str) -> Result<BTreeSet<String>> {
    json_io::array(value, label)?
        .iter()
        .map(|item| Ok(json_io::text(item, label)?.to_owned()))
        .collect()
}
