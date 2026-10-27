use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{
    TOOLS, command, digest, native_json, normalize_cache_path, path_text, recorded_cargo_path,
    source, target_directory,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

pub(super) fn verify_evidence(
    backend: &Path,
    filename: &Path,
    expected: Option<&Value>,
) -> Result<Value> {
    validate_local_test_path(filename, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("维护构建收据无效：{error}"))?;
    if filename.file_name().and_then(|name| name.to_str()) != Some("build.json")
        || filename
            .parent()
            .is_some_and(|parent| parent.join("failed.json").exists())
    {
        return Err("维护工具构建失败或收据名称不匹配".into());
    }
    let before = digest::file(filename)?;
    let receipt = native_json::read(filename)?;
    if expected.is_some_and(|value| value != &receipt) {
        return Err("维护工具构建收据与调用方绑定内容不同".into());
    }
    verify_receipt(backend, filename, &receipt)?;
    if digest::file(filename)? != before || native_json::read(filename)? != receipt {
        return Err("校验维护工具期间收据发生变化".into());
    }
    Ok(receipt)
}

fn verify_receipt(backend: &Path, filename: &Path, receipt: &Value) -> Result<()> {
    let fields = [
        "format_version",
        "kind",
        "backend_root",
        "source",
        "toolchain",
        "target_directory",
        "artifacts",
        "restore_qualified",
        "resources_modified",
        "source_inventory",
    ];
    let present = fields
        .into_iter()
        .filter(|field| *field != "source_inventory" || receipt.get(*field).is_some())
        .collect::<Vec<_>>();
    native_json::exact(receipt, &present, "维护工具构建收据")?;
    let target = target_directory(backend)?;
    if receipt["format_version"].as_u64() != Some(1)
        || receipt["kind"].as_str() != Some("devex-clone-tool-build")
        || receipt["backend_root"].as_str() != Some(path_text(backend, "后端根目录")?.as_str())
        || receipt["target_directory"].as_str()
            != Some(path_text(&target, "维护构建 target")?.as_str())
        || receipt["restore_qualified"].as_bool() != Some(false)
        || receipt["resources_modified"].as_bool() != Some(false)
    {
        return Err("维护工具构建证据格式或用途不符".into());
    }
    let artifacts = native_json::object(
        receipt.get("artifacts").ok_or("维护工具构建收据缺少产物")?,
        "维护工具构建产物",
    )?;
    let expected = TOOLS
        .iter()
        .map(|(role, _, _)| (*role).to_owned())
        .collect::<BTreeSet<_>>();
    if artifacts.keys().cloned().collect::<BTreeSet<_>>() != expected {
        return Err("维护工具构建证据格式或用途不符".into());
    }
    if let Some(inventory) = receipt.get("source_inventory") {
        source::validate_inventory(inventory)?;
    }
    for (role, _, name) in TOOLS {
        let item = artifacts.get(role).ok_or("维护工具构建收据缺少角色产物")?;
        verify_artifact(backend, filename, &target, role, name, item)?;
    }
    Ok(())
}

fn verify_artifact(
    backend: &Path,
    filename: &Path,
    target: &Path,
    role: &str,
    name: &str,
    item: &Value,
) -> Result<()> {
    let item = native_json::exact(
        item,
        &[
            "executable",
            "cargo_executable",
            "command",
            "cargo_output",
            "cargo_log",
            "bytes",
            "sha256",
        ],
        "维护工具构建产物",
    )?;
    let executable = path_value(item.get("executable").unwrap(), "维护工具路径")?;
    validate_local_test_path(&executable, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("维护工具路径无效：{error}"))?;
    if executable.parent() != filename.parent()
        || item.get("command").and_then(Value::as_array) != Some(&command_values(role, target)?)
    {
        return Err("维护工具路径或定向构建命令不匹配".into());
    }
    if digest::file(&executable)?
        != (digest::FileDigest {
            bytes: item["bytes"].as_u64().ok_or("维护工具字节数无效")?,
            sha256: item["sha256"]
                .as_str()
                .ok_or("维护工具摘要无效")?
                .to_owned(),
        })
    {
        return Err("维护工具实际二进制与收据不一致".into());
    }
    verify_log(
        filename,
        role,
        "cargo_output",
        "jsonl",
        item.get("cargo_output").unwrap(),
    )?;
    verify_log(
        filename,
        role,
        "cargo_log",
        "log",
        item.get("cargo_log").unwrap(),
    )?;
    let output = filename
        .parent()
        .expect("收据具有父目录")
        .join(format!("{role}.cargo.jsonl"));
    let recorded = recorded_cargo_path(backend, name, &fs::read_to_string(output)?)?;
    let declared = normalize_cache_path(&path_value(
        item.get("cargo_executable").unwrap(),
        "Cargo 原始产物",
    )?)?;
    if recorded != declared || !recorded.starts_with(normalize_cache_path(target)?) {
        return Err("维护工具 Cargo 事件与实际构建路径不一致".into());
    }
    Ok(())
}

fn verify_log(
    filename: &Path,
    role: &str,
    field: &str,
    extension: &str,
    value: &Value,
) -> Result<()> {
    let binding = native_json::exact(value, &["file", "bytes", "sha256"], "维护工具日志绑定")?;
    let expected = format!("{role}.cargo.{extension}");
    if binding.get("file").and_then(Value::as_str) != Some(expected.as_str()) {
        return Err("维护工具日志路径与角色不匹配".into());
    }
    let path = filename.parent().expect("收据具有父目录").join(expected);
    if digest::file(&path)?
        != (digest::FileDigest {
            bytes: binding["bytes"].as_u64().ok_or("维护工具日志字节数无效")?,
            sha256: binding["sha256"]
                .as_str()
                .ok_or("维护工具日志摘要无效")?
                .to_owned(),
        })
    {
        return Err(format!("维护工具 {field} 构建日志发生变化").into());
    }
    Ok(())
}

fn command_values(role: &str, target: &Path) -> Result<Vec<Value>> {
    Ok(command(role, target)?
        .into_iter()
        .map(Value::String)
        .collect())
}

fn path_value(value: &Value, label: &str) -> Result<PathBuf> {
    let path = PathBuf::from(native_json::text(value, label)?);
    if !path.is_absolute() {
        return Err(format!("{label}必须是绝对路径").into());
    }
    Ok(path)
}
