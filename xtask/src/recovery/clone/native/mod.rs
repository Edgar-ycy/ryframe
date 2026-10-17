use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::CloneCommand,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

mod business;
mod catalog;
mod digest;
mod input;
mod json;
mod metadata;
mod physical;
mod plan;
pub(crate) mod schedule;
pub(crate) mod sql;
mod state;

pub(crate) fn run(command: &CloneCommand, backend: &Path) -> Result<()> {
    let value = match command {
        CloneCommand::Plan { input, output } => plan(backend, input, output)?,
        CloneCommand::Verify { plan } => verify(backend, plan)?,
        _ => return Err("clone 原生模块不处理该命令".into()),
    };
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

fn plan(backend: &Path, input: &Path, output: &Path) -> Result<Value> {
    validate_local_test_path(input, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("复制输入无效：{error}"))?;
    validate_local_test_path(output, backend, LocalTestPathKind::OutputFile)
        .map_err(|error| format!("复制计划输出无效：{error}"))?;
    let declaration = json::read(input)?;
    let artifact_root = PathBuf::from(
        declaration["artifact_root"]
            .as_str()
            .ok_or("复制证据根目录必须为字符串")?,
    );
    let artifact_root = validate_local_test_path(
        &artifact_root,
        backend,
        LocalTestPathKind::ExistingDirectory,
    )
    .map_err(|error| format!("复制证据根目录无效：{error}"))?;
    if output.starts_with(&artifact_root) {
        return Err("计划输出不得写入来源导出目录".into());
    }
    let (wrapper, _) = inspect_input(backend, input)?;
    json::write_new(output, &wrapper)?;
    let value = issue(backend, output, &wrapper)?;
    summary(&value)
}

fn verify(backend: &Path, plan: &Path) -> Result<Value> {
    validate_local_test_path(plan, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("复制计划无效：{error}"))?;
    let before = digest::file(plan)?;
    let wrapper = json::read(plan)?;
    let object = json::exact(
        &wrapper,
        &["format_version", "input_path", "input_file", "plan"],
        "已写复制计划",
    )?;
    if object.get("format_version").and_then(Value::as_u64) != Some(1) {
        return Err("已写复制计划版本无效".into());
    }
    if object.get("input_path").and_then(Value::as_str).is_none() {
        return Err("已写复制计划缺少 input_path".into());
    }
    if digest::file(plan)? != before {
        return Err("核验期间计划文件变化".into());
    }
    let value = issue(backend, plan, &wrapper)?;
    summary(&value)
}

fn inspect_input(backend: &Path, input: &Path) -> Result<(Value, Vec<PathBuf>)> {
    validate_local_test_path(input, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("复制输入无效：{error}"))?;
    let before = digest::file(input)?;
    let declaration = json::read(input)?;
    let output = plan::create(&declaration, backend)?;
    if digest::file(input)? != before {
        return Err("离线核验期间输入清单变化".into());
    }
    let wrapper = json!({
        "format_version": 1,
        "input_path": digest::path_text(input, "复制输入")?,
        "input_file": {
            "bytes": before.bytes,
            "sha256": before.sha256,
        },
        "plan": output.value,
    });
    Ok((wrapper, output.sources))
}

pub(crate) fn issue(backend: &Path, plan: &Path, wrapper: &Value) -> Result<Value> {
    let before = digest::file(plan)?;
    if json::read(plan)? != *wrapper {
        return Err("完整验证结果签发前计划文件变化".into());
    }
    let input = wrapper["input_path"]
        .as_str()
        .ok_or("计划缺少完整输入路径")?;
    let (actual, _) = inspect_input(backend, Path::new(input))?;
    if actual != *wrapper {
        return Err("已写计划与当前完整输入、源码策略或导出证据不一致".into());
    }
    if digest::file(plan)? != before {
        return Err("完整验证结果签发前计划文件变化".into());
    }
    Ok(wrapper.clone())
}

fn summary(value: &Value) -> Result<Value> {
    Ok(json!({
        "status": value["plan"]["status"].clone(),
        "plan_sha256": value["plan"]["plan_sha256"].clone(),
        "pending_target_actions": value["plan"]["pending_target_actions"]
            .as_array()
            .ok_or("计划缺少目标调度待办列表")?
            .len(),
        "target_ready": false,
        "worker_must_remain_stopped": true,
        "execution_authorized": false,
        "resources_modified": false,
    }))
}
