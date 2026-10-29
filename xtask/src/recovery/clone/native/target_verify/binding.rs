//! fresh CLI 的只读绑定协议；敏感值仅由已绑定文件读取，错误只公开固定说明。

use std::{collections::BTreeMap, fs, path::Path};

use serde_json::{Value, json};

use super::super::{digest, json as json_io};
use super::{
    binding_request,
    evidence_model::{binding, bound, check, read},
    input::request_review,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
};

const DIAGNOSTICS: &[&str] = &[
    "fresh 目标私有环境只允许 APP_ENV、TEMP 和 TMP；请将其他配置写入绑定 TOML 文件",
    "fresh 目标私有环境存在大小写重复、非文本或空字段；请修正环境文件",
    "fresh 目标 TEMP 与 TMP 必须绑定同一绝对私有目录；请检查环境文件和目录",
    "fresh 目标私有环境在读取期间变化；请保留现场并重新核对绑定",
    "fresh 目标预检失败收尾收据无效；请核对 failure 与唯一 reset report，禁止重放初始化",
    "fresh 目标预检失败收尾资源前后像无效；请核对完整资源证据，禁止自动清理",
    "目标 config_dir 必须是已绑定执行树内的无链接配置目录；请修正请求 config_dir",
    "目标已经出现生产者启动意图或进程收据，不能认定为从未启动的 fresh 目标",
    "目标已经出现运行或身份准备历史，不具备本代次 fresh 条件",
];

pub(super) fn diagnostic(reason: &str) -> Option<&'static str> {
    DIAGNOSTICS
        .iter()
        .copied()
        .find(|message| *message == reason)
        .or_else(|| super::binding_configuration::diagnostic(reason))
        .or_else(|| binding_request::diagnostic(reason))
}

pub(super) fn execute(bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() || bytes.len() > 1024 * 1024 {
        return Err("目标绑定请求为空或超过 1 MiB".into());
    }
    let value = json_io::parse_fragment(std::str::from_utf8(bytes)?, "目标绑定请求")?;
    json_io::exact(
        &value,
        &["format_version", "operation", "backend", "arguments"],
        "目标绑定请求",
    )?;
    check(
        value["format_version"].as_u64() == Some(1),
        "目标绑定请求版本无效",
    )?;
    let backend = Path::new(json_io::text(&value["backend"], "目标绑定后端")?);
    validate_absolute_path(backend, LocalTestPathKind::ExistingDirectory)?;
    dispatch(
        backend,
        json_io::text(&value["operation"], "目标绑定操作")?,
        &value["arguments"],
    )
}

fn dispatch(backend: &Path, operation: &str, args: &Value) -> Result<Value> {
    match operation {
        "fresh-status" | "fresh-prepare" | "fresh-resume-prepare" | "fresh-verify-target" => {
            super::fresh::binding(backend, operation, args)
        }
        "reset-configuration" | "redis-configuration" => super::binding_configuration::validate(
            backend,
            args,
            operation == "reset-configuration",
        ),
        "review" => {
            json_io::exact(args, &["review"], "审阅请求")?;
            request_review::validate(&args["review"])?;
            Ok(json!({"status":"review_verified"}))
        }
        "external-file" => {
            json_io::exact(args, &["descriptor"], "外部工具绑定")?;
            super::input::request::external_file(&args["descriptor"])?;
            Ok(json!({"status":"external_file_verified"}))
        }
        "request" => {
            json_io::exact(args, &["request", "predecessor"], "目标请求绑定")?;
            binding_request::validate(backend, &args["request"], &args["predecessor"])
        }
        "binaries" => {
            json_io::exact(args, &["request"], "目标二进制绑定")?;
            binding_request::binaries(backend, &args["request"])
        }
        "private-environment" => {
            json_io::exact(args, &["descriptor"], "目标私有环境绑定")?;
            private_environment(backend, &args["descriptor"])
        }
        "preflight-reconciliation" => {
            json_io::exact(args, &["target"], "目标收尾绑定")?;
            reconciliation(
                backend,
                Path::new(json_io::text(&args["target"], "目标收尾路径")?),
            )
        }
        "config-directory" => {
            json_io::exact(args, &["request"], "目标配置目录绑定")?;
            super::input::request::config_directory(backend, &args["request"])
        }
        "never-started" => {
            json_io::exact(args, &["selected"], "目标运行历史绑定")?;
            never_started(backend, &args["selected"])?;
            Ok(json!({"status":"never_started_verified"}))
        }
        "reset-manifest" => {
            json_io::exact(
                args,
                &["manifest", "request", "original"],
                "目标 reset 清单绑定",
            )?;
            super::evidence_reset::validate_manifest(
                backend,
                &args["manifest"],
                &args["request"],
                &args["original"],
            )?;
            Ok(json!({"status":"reset_manifest_verified"}))
        }
        _ => Err("未知目标绑定操作；请使用当前受管工具和协议".into()),
    }
}

pub(super) fn private_environment(backend: &Path, descriptor: &Value) -> Result<Value> {
    let path = bound(backend, descriptor)?;
    let document = read(backend, &path)?;
    json_io::exact(&document, &["environment"], "目标私有环境")?;
    let private = json_io::object(&document["environment"], "目标私有环境字段")?;
    let mut folded = BTreeMap::new();
    for (key, value) in private {
        let upper = key.to_ascii_uppercase();
        check(
            matches!(upper.as_str(), "APP_ENV" | "TEMP" | "TMP"),
            DIAGNOSTICS[0],
        )?;
        let text = value
            .as_str()
            .filter(|value| !value.is_empty() && !value.contains('\0'));
        check(
            text.is_some() && folded.insert(upper, text).is_none(),
            DIAGNOSTICS[1],
        )?;
    }
    let temporary = folded
        .get("TEMP")
        .copied()
        .flatten()
        .ok_or(DIAGNOSTICS[2])?;
    check(
        folded.get("TMP").copied().flatten() == Some(temporary),
        DIAGNOSTICS[2],
    )?;
    validate_local_test_path(
        Path::new(temporary),
        backend,
        LocalTestPathKind::ExistingDirectory,
    )
    .map_err(|_| DIAGNOSTICS[2])?;
    check(binding(backend, &path)? == *descriptor, DIAGNOSTICS[3])?;
    Ok(json!({"path":path,"environment":private}))
}

pub(super) fn never_started(backend: &Path, selected: &Value) -> Result<()> {
    let runtime = Path::new(json_io::text(&selected["runtime_dir"], "目标运行目录")?);
    validate_local_test_path(runtime, backend, LocalTestPathKind::StateDirectory)?;
    for filename in ["producer-history.json", "api.json", "worker.json"] {
        let evidence = runtime.join(filename);
        match fs::symlink_metadata(&evidence) {
            Ok(_) => return Err(DIAGNOSTICS[7].into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(DIAGNOSTICS[7].into()),
        }
    }
    for field in ["runtime_dir", "identity_ledger"] {
        let path = Path::new(json_io::text(&selected[field], "目标运行历史目录")?);
        validate_local_test_path(path, backend, LocalTestPathKind::StateDirectory)?;
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                check(
                    !digest::linked(&metadata)
                        && metadata.is_dir()
                        && fs::read_dir(path)?.next().is_none(),
                    DIAGNOSTICS[8],
                )?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(super) fn reconciliation(backend: &Path, target: &Path) -> Result<Value> {
    validate_local_test_path(target, backend, LocalTestPathKind::ExistingDirectory)?;
    let completed = target.join("reconciliation-completed.json");
    let descriptor = binding(backend, &completed)?;
    let value = read(backend, &completed)?;
    json_io::exact(
        &value,
        &[
            "status",
            "before",
            "after",
            "failure",
            "reset_report",
            "automatic_retry",
            "restore_qualified",
        ],
        "预检收尾收据",
    )?;
    let mut reports = Vec::new();
    let reports_directory = target.join("reset-state");
    validate_local_test_path(
        &reports_directory,
        backend,
        LocalTestPathKind::ExistingDirectory,
    )?;
    for entry in fs::read_dir(reports_directory)? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".report.json"))
        {
            reports.push(path);
        }
    }
    check(
        value["status"] == "preflight_failure_reconciled"
            && value["automatic_retry"].as_bool() == Some(false)
            && value["restore_qualified"].as_bool() == Some(false)
            && reports.len() == 1,
        DIAGNOSTICS[4],
    )?;
    for (field, path) in [
        ("failure", target.join("failure.json")),
        ("reset_report", reports[0].clone()),
    ] {
        let mut actual = binding(backend, &path)?;
        actual.as_object_mut().ok_or(DIAGNOSTICS[4])?.remove("path");
        check(value[field] == actual, DIAGNOSTICS[4])?;
    }
    for (field, exists) in [("before", true), ("after", false)] {
        let state = json_io::exact(
            &value[field],
            &["databases", "objects", "redis"],
            "收尾资源前后像",
        )?;
        let databases = json_io::array(&state["databases"], "收尾数据库前后像")?;
        check(
            !databases.is_empty()
                && databases
                    .iter()
                    .all(|item| item["exists"].as_bool() == Some(exists)),
            DIAGNOSTICS[5],
        )?;
    }
    check(binding(backend, &completed)? == descriptor, DIAGNOSTICS[4])?;
    Ok(json!({"binding":descriptor}))
}
