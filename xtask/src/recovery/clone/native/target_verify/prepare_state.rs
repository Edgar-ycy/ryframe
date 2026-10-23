//! 只读 prepare 续作判定；任何资源写入意图均不能由本入口授权重放。
use std::{collections::BTreeMap, fs, path::Path};

use regex::Regex;
use serde_json::{Value, json};

use super::{
    super::{digest, json as json_io},
    evidence_model::{binding, bound, check, read, timestamp},
    evidence_prepare,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const PREPARE: &[&str] = &[
    "format_version",
    "status",
    "prepared_at",
    "request",
    "request_sha256",
    "generation",
    "absence",
    "fresh_target_initialized",
    "controlled_generation_only",
    "external_writers_discovered",
    "create_intents",
];

pub(super) fn failure(backend: &Path, target: &Path) -> Result<Option<Value>> {
    let path = target.join("failure.json");
    if !path.try_exists()? {
        return Ok(None);
    }
    let value = read(backend, &path)?;
    json_io::exact(&value, evidence_prepare::FAILURE, "prepare 失败")?;
    check(
        value["status"] == "needs_reconciliation"
            && value["stage"].as_str().is_some_and(|stage| {
                ["request", "prepare_binding", "prepare_absence"].contains(&stage)
            })
            && timestamp(&value["at"]).is_ok()
            && ["error_type", "reason"]
                .iter()
                .all(|key| value[*key].as_str().is_some_and(|text| !text.is_empty()))
            && [
                "automatic_retry",
                "automatic_resource_cleanup",
                "fresh_target_initialized",
                "clone_verified",
                "restore_qualified",
            ]
            .iter()
            .all(|key| value[*key] == false),
        "fresh 目标 failure 不是可续作的只读 prepare 失败",
    )?;
    Ok(Some(value))
}

fn result(
    resumable: bool,
    prepared: bool,
    reason: Option<&str>,
    files: Value,
    pending: Value,
) -> Value {
    json!({"resumable":resumable,"prepared":prepared,"reason":reason,"files":files,"pending_attempts":pending})
}

pub(super) fn state(backend: &Path, target: &Path, request: &Value) -> Result<Value> {
    bound(backend, request)?;
    validate_local_test_path(target, backend, LocalTestPathKind::StateDirectory)?;
    if !target.try_exists()? {
        return Ok(result(true, false, None, json!({}), json!([])));
    }
    let files = scan(backend, target)?;
    if let Some(reason) = files.0 {
        return Ok(result(
            false,
            false,
            Some(reason),
            json!(files.1),
            json!([]),
        ));
    }
    let requested = target.join("request.json");
    if requested.try_exists()? {
        check(
            read(backend, &requested)? == read(backend, &bound(backend, request)?)?,
            "fresh 目标中断请求不同于固定 registration",
        )?;
    }
    let failed = failure(backend, target)?;
    let records = evidence_prepare::records(backend, target, request)?;
    let prepared = target.join("prepare.json");
    let exists = prepared.try_exists()?;
    let terminals = json_io::object(&records["terminals"], "prepare 结果")?;
    if exists {
        let value = read(backend, &prepared)?;
        json_io::exact(&value, PREPARE, "prepare 文件")?;
        check(
            value["status"] == "fresh_creation_prepared" && value["request"] == *request,
            "fresh 目标 prepare 结果不属于固定 registration",
        )?;
        let confirmed = terminals
            .values()
            .any(|terminal| terminal[0] == "confirmed");
        check(
            failed.is_none()
                || confirmed
                || !json_io::array(&records["pending"], "prepare 待结束续作")?.is_empty(),
            "fresh 目标 prepare 与旧失败之间缺少显式续作绑定",
        )?;
    } else {
        check(
            !terminals
                .values()
                .any(|terminal| terminal[0] == "confirmed"),
            "fresh 目标 prepare 续作确认缺少最终 prepare.json",
        )?;
    }
    Ok(result(
        true,
        exists,
        None,
        json!(files.1),
        records["pending"].clone(),
    ))
}

fn scan(backend: &Path, target: &Path) -> Result<(Option<&'static str>, BTreeMap<String, Value>)> {
    let mut entries = fs::read_dir(target)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let write = Regex::new(r"^create-.*\.intent\.json$")?;
    if entries.iter().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        matches!(name.as_ref(), "sentinel.intent.json" | "reset.intent.json")
            || write.is_match(&name)
    }) {
        return Ok((Some("已存在资源写入 intent"), BTreeMap::new()));
    }
    let mut files = BTreeMap::new();
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "prepare 文件名无效")?;
        if !entry.file_type()?.is_file() || !evidence_prepare::artifact(&name)? {
            return Ok((Some("包含非只读 prepare 证据"), files));
        }
        let path = entry.path();
        binding(backend, &path)?;
        let value = digest::file(&path)?;
        files.insert(name, json!({"bytes":value.bytes,"sha256":value.sha256}));
    }
    Ok((None, files))
}

pub(super) fn resolved(backend: &Path, target: &Path, request: &Value) -> bool {
    (|| -> Result<bool> {
        if failure(backend, target)?.is_none() || !target.join("prepare.json").is_file() {
            return Ok(false);
        }
        let records = evidence_prepare::records(backend, target, request)?;
        Ok(
            json_io::array(&records["pending"], "prepare 待结束续作")?.is_empty()
                && json_io::object(&records["terminals"], "prepare 结果")?
                    .values()
                    .any(|value| value[0] == "confirmed"),
        )
    })()
    .unwrap_or(false)
}
