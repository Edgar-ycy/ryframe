use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::{Value, json};

use super::{
    super::{input::BUCKETS, json as document, resources::common},
    receipts,
};
use crate::Result;

pub(super) fn inspect(backend: &Path, output: &Path, target: &Value) -> Result<Value> {
    common::local_directory(backend, output, "部门守卫目录")?;
    let identity = common::device_inode_for(output, &fs::symlink_metadata(output)?)?;
    let mut entries = fs::read_dir(output)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    let command =
        Regex::new(r"^(?:command-[a-f0-9]{32}|redis-kernel-[a-f0-9]{32}\.command)\.json$")?;
    let mysql = Regex::new(r"^mysql-(identity|ownership)-([a-f0-9]{32})\.(json|stdout)$")?;
    let retry = Regex::new(r"^mysql-(identity|ownership)-retry-[a-f0-9]{32}\.json$")?;
    let owner = Regex::new(r"^owner-target-([a-z0-9-]+)-[a-f0-9]{32}\.txt$")?;
    let layout = Regex::new(r"^storage-layout-[a-f0-9]{32}\.json$")?;
    let mut artifacts = Vec::new();
    let mut payloads = BTreeSet::new();
    let mut referenced = BTreeSet::new();
    for path in entries {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("部门守卫文件名不是 UTF-8")?;
        if name == "adoption-guard.json" {
            continue;
        }
        common::common_file(backend, &path, "部门守卫证据")?;
        let before = common::binding(&path)?;
        if command.is_match(name) {
            command_artifact(backend, output, &path, target, &mut referenced)?;
        } else if let Some(matches) = mysql.captures(name) {
            if &matches[3] == "stdout" {
                payloads.insert(path.clone());
            } else {
                let stdout = mysql_artifact(backend, &path, &matches[1])?;
                referenced.insert(stdout);
            }
        } else if let Some(matches) = retry.captures(name) {
            retry_artifact(backend, output, &path, &matches[1], &mysql)?;
        } else if let Some(matches) = owner.captures(name) {
            owner_artifact(&path, &matches[1], target)?;
            payloads.insert(path.clone());
        } else if layout.is_match(name) {
            let value = document::read(&path)?;
            document::exact(
                &value,
                &[
                    "actual_arguments_sha256",
                    "data_dir",
                    "environment_proof",
                    "launch_receipt",
                    "process_receipt",
                    "runtime_transition",
                ],
                "部门存储布局",
            )?;
            common::bound_file(backend, &value["launch_receipt"], "部门布局启动收据")?;
            common::bound_file(backend, &value["process_receipt"], "部门布局进程收据")?;
        } else {
            return Err("部门采用链节点出现动作、目录或未知证据".into());
        }
        if common::binding(&path)? != before {
            return Err("部门守卫证据在核验期间变化".into());
        }
        artifacts.push(before);
    }
    if payloads != referenced {
        return Err("部门守卫本地输出没有被只读命令收据完整覆盖".into());
    }
    validate_closed(backend, output, &artifacts, identity)?;
    Ok(json!(artifacts))
}

fn validate_closed(
    backend: &Path,
    output: &Path,
    artifacts: &[Value],
    identity: (u64, u64),
) -> Result<()> {
    common::local_directory(backend, output, "部门守卫目录")?;
    if common::device_inode_for(output, &fs::symlink_metadata(output)?)? != identity {
        return Err("部门守卫目录身份在核验期间变化".into());
    }
    let mut actual = fs::read_dir(output)?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    actual.retain(|name| name != "adoption-guard.json");
    actual.sort();
    let mut expected = Vec::new();
    for descriptor in artifacts {
        let path = common::bound_file(backend, descriptor, "部门守卫最终证据")?;
        expected.push(
            path.file_name()
                .ok_or("部门守卫证据名称缺失")?
                .to_os_string(),
        );
    }
    if actual != expected {
        return Err("部门守卫目录闭集在核验期间变化".into());
    }
    Ok(())
}

fn command_artifact(
    backend: &Path,
    output: &Path,
    path: &Path,
    target: &Value,
    referenced: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let value = document::read(path)?;
    receipts::read_only(&value, target)?;
    if value["transport"] == "rust-s3-sigv4" {
        if value["operation"] == "get-object" {
            let destination =
                output.join(document::text(&value["destination"], "部门 owner 输出")?);
            if !referenced.insert(destination) {
                return Err("部门守卫对象输出被多份收据重复声明".into());
            }
        }
        return Ok(());
    }
    if let Some(descriptor) = value.get("stdout_file") {
        let stdout = common::bound_file(backend, descriptor, "部门守卫 stdout")?;
        if stdout.parent() != Some(output) {
            return Err("部门守卫 stdout 跨出当前 attempt".into());
        }
        referenced.insert(stdout);
    }
    for argument in document::array(&value["command"], "部门守卫命令")? {
        let candidate = Path::new(document::text(argument, "部门守卫参数")?);
        if candidate.is_absolute() && candidate.parent() == Some(output) {
            referenced.insert(candidate.to_path_buf());
        }
    }
    Ok(())
}

fn mysql_artifact(backend: &Path, path: &Path, check: &str) -> Result<PathBuf> {
    let value = document::read(path)?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "check",
            "target_key",
            "returncode",
            "error_type",
            "stderr_bytes",
            "stderr_sha256",
            "stdout_file",
        ],
        "部门 MySQL 核验收据",
    )?;
    let stdout = path.with_extension("stdout");
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "mysql-verification-output"
        || value["check"] != check
        || value["returncode"].as_i64() != Some(0)
        || !value["error_type"].is_null()
        || value["stderr_bytes"].as_u64() != Some(0)
        || value["stderr_sha256"]
            != "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        || common::bound_file(backend, &value["stdout_file"], "部门 MySQL stdout")? != stdout
    {
        return Err("部门守卫 MySQL 核验收据无效".into());
    }
    Ok(stdout)
}

fn retry_artifact(
    backend: &Path,
    output: &Path,
    path: &Path,
    check: &str,
    pattern: &Regex,
) -> Result<()> {
    let value = document::read(path)?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "check",
            "target_key",
            "reason",
            "first",
            "second",
        ],
        "部门 MySQL 重试",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "mysql-verification-retry"
        || value["check"] != check
        || value["reason"] != "first_returncode_zero_stdout_stderr_exactly_empty"
    {
        return Err("部门守卫 MySQL 空输出重试收据无效".into());
    }
    for field in ["first", "second"] {
        let receipt = common::bound_file(backend, &value[field], "部门 MySQL 重试收据")?;
        if receipt.parent() != Some(output)
            || !receipt
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|name| pattern.is_match(name))
        {
            return Err("部门守卫 MySQL 重试跨出当前 attempt".into());
        }
    }
    Ok(())
}

fn owner_artifact(path: &Path, bucket: &str, target: &Value) -> Result<()> {
    if !BUCKETS.contains(&bucket) {
        return Err("部门守卫 owner 桶无效".into());
    }
    let scope = document::text(&target["scope_id"], "部门 owner scope")?;
    common::scope_id(scope)?;
    let expected = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
    if fs::read(path)? != expected.as_bytes() {
        return Err("部门守卫 owner 内容无效".into());
    }
    Ok(())
}
