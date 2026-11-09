use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::FixtureSourcePairCommand,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::json::exact,
};

use super::{
    exact_object, file_binding, git_root, path_text_canonical, portable_path, print_json,
    read_json, text_field, valid_sha256, write_new,
};

#[path = "source_pair/execution.rs"]
pub(crate) mod execution;

const MAX_RECEIPT_BYTES: u64 = 4 * 1024 * 1024;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

pub(super) fn run(command: &FixtureSourcePairCommand, backend: &Path) -> Result<()> {
    if let FixtureSourcePairCommand::PublishExecution { output_dir } = command {
        let receipt = execution::publish(backend, output_dir)?;
        return print_json(&receipt);
    }
    let FixtureSourcePairCommand::Publish(options) = command else {
        println!(
            "cargo xtask check recovery fixture source-pair --output <新文件> --write\ncargo xtask check recovery fixture source-pair publish-execution --output-dir <新目录> --write"
        );
        return Ok(());
    };
    publish(backend, &options.output)?;
    print_json(&json!({
        "status": "reference_fixture_source_pair_created",
        "remote_writes": 0,
    }))
}

pub(crate) fn publish(backend: &Path, output: &Path) -> Result<Value> {
    validate_local_test_path(output, backend, LocalTestPathKind::OutputFile)
        .map_err(|error| format!("source-pair 输出无效：{error}"))?;
    let backend = portable_path(
        &fs::canonicalize(backend)
            .map_err(|error| format!("无法解析当前后端 {}：{error}", backend.display()))?,
    );
    let fixture_root = backend.parent().ok_or("参考夹具后端缺少 fixture 根目录")?;
    let fixture_path = fixture_root.join("fixture.json");
    let fixture = read_json(&fixture_path, "业务 crate fixture 收据", MAX_RECEIPT_BYTES)?;
    let root = exact_object(
        &fixture,
        &[
            "format_version",
            "fixture",
            "status",
            "fixture_sha256",
            "sources",
            "paths",
            "generated",
        ],
        "业务 crate fixture 收据",
    )?;
    if root.get("format_version").and_then(Value::as_u64) != Some(1)
        || root.get("fixture").and_then(Value::as_str) != Some("business")
        || root.get("status").and_then(Value::as_str) != Some("ready")
    {
        return Err("业务 crate fixture 尚未完成或格式不受支持".into());
    }
    let paths = exact_object(
        root.get("paths").ok_or("fixture 收据缺少 paths")?,
        &["backend", "frontend"],
        "业务 crate fixture 工作树路径",
    )?;
    let planned_backend = PathBuf::from(
        paths
            .get("backend")
            .and_then(Value::as_str)
            .ok_or("业务 crate 后端工作树路径无效")?,
    );
    validate_fixture_root(&planned_backend, fixture_root, &backend)?;
    let sources = exact_object(
        root.get("sources").ok_or("fixture 收据缺少 sources")?,
        &["backend", "frontend"],
        "业务 crate 原始来源",
    )?;
    for name in ["backend", "frontend"] {
        validate_clean_snapshot(
            sources.get(name).ok_or("fixture 原始来源缺少侧")?,
            &format!("业务 crate {name} 原始来源"),
        )?;
    }
    let receipt = json!({
        "format_version": 1,
        "kind": "reference-fixture-source-pair",
        "fixture_receipt": file_binding(&fixture_path, "业务 crate fixture 收据")?,
        "sources": Value::Object(sources.clone()),
    });

    validate_output(output, &backend)?;
    if read_json(&fixture_path, "业务 crate fixture 收据", MAX_RECEIPT_BYTES)? != fixture
        || receipt.get("fixture_receipt")
            != Some(&file_binding(&fixture_path, "业务 crate fixture 收据")?)
    {
        return Err("业务 crate fixture 收据在来源组合构造期间发生变化".into());
    }
    write_new(output, &receipt)?;
    if read_json(output, "参考夹具来源组合收据", MAX_RECEIPT_BYTES)? != receipt {
        return Err("参考夹具来源组合收据发布后无法重算".into());
    }
    Ok(receipt)
}

fn validate_fixture_root(planned: &Path, fixture_root: &Path, backend: &Path) -> Result<()> {
    let resolved = portable_path(
        &fs::canonicalize(planned)
            .map_err(|error| format!("业务 crate 后端工作树不存在或无法解析：{error}"))?,
    );
    if !planned.is_absolute()
        || planned != resolved
        || resolved.parent() != Some(fixture_root)
        || !resolved.is_dir()
        || git_root(&resolved, "业务 crate 后端工作树")? != resolved
        || resolved != backend
    {
        return Err("业务 crate fixture 与当前后端工作树不匹配".into());
    }
    Ok(())
}

fn validate_clean_snapshot(value: &Value, label: &str) -> Result<()> {
    let snapshot = exact(value, &["head", "patch_sha256", "files"], label)?;
    let head = text_field(value, "head", label)?;
    if !regex::Regex::new(r"^[a-f0-9]{40}$")?.is_match(head) {
        return Err(format!("{label} HEAD 不是有效提交 SHA").into());
    }
    let patch = text_field(value, "patch_sha256", label)?;
    valid_sha256(patch, &format!("{label}补丁摘要"))?;
    let files = snapshot
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{label}未跟踪文件清单无效"))?;
    if patch != EMPTY_SHA256 || !files.is_empty() {
        return Err(format!("{label}必须是干净源码").into());
    }
    Ok(())
}

fn validate_output(output: &Path, backend: &Path) -> Result<()> {
    let allowed = backend.join(".local-tests").join("reference-fixture");
    let parent = output.parent().ok_or("参考夹具来源组合输出缺少父目录")?;
    if output.file_name().and_then(|value| value.to_str()) != Some("source-pair.json")
        || !parent.starts_with(&allowed)
        || !parent.is_dir()
    {
        return Err("参考夹具来源组合必须写入执行工作树的指定新 source-pair.json".into());
    }
    path_text_canonical(parent, "source-pair 输出父目录")?;
    if output.exists() || fs::symlink_metadata(output).is_ok() {
        return Err("参考夹具来源组合输出已存在，拒绝覆盖".into());
    }
    Ok(())
}
