use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::{CloneCommand, CloneRuntimeOperation},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

pub(crate) mod bridge;
pub(crate) mod business;
pub(crate) mod cache;
pub(crate) mod catalog;
pub(crate) mod copy_ledger;
pub(crate) mod copy_owner;
mod copy_recover;
pub(crate) mod copy_transfer;
pub(crate) mod department;
pub(crate) mod digest;
pub(crate) mod export;
pub(crate) mod input;
pub(crate) mod json;
pub(crate) mod lifecycle;
pub(crate) mod maintenance;
pub(crate) mod maintenance_root;
pub(crate) mod metadata;
mod physical;
mod plan;
pub(crate) mod post_process;
pub(crate) mod resources;
pub(crate) mod s3;
pub(crate) mod schedule;
pub(crate) mod seed;
pub(crate) mod session;
pub(crate) mod source;
pub(crate) mod sql;
pub(crate) mod sql_stream;
mod state;
pub(crate) mod target_verify;

pub(crate) fn run(command: &CloneCommand, backend: &Path) -> Result<()> {
    let value = match command {
        CloneCommand::Plan { input, output } => plan(backend, input, output)?,
        CloneCommand::Verify { plan } => verify(backend, plan)?,
        CloneCommand::Init { .. }
        | CloneCommand::Status { .. }
        | CloneCommand::Stage { .. }
        | CloneCommand::Recover { .. } => lifecycle::run(command, backend)?,
        CloneCommand::RecoverCopy {
            run_dir,
            owner_binding,
        } => copy_recover::run(backend, run_dir, owner_binding)?,
        CloneCommand::Bridge {
            build,
            inventory,
            output,
        } => bridge::write(backend, build, inventory, output)?,
        CloneCommand::MaintenanceBuild { output } => {
            maintenance::build(backend, output)?;
            maintenance::build_summary(output)
        }
        CloneCommand::MaintenanceVerify { output } => {
            let receipt = if output.is_dir() {
                output.join("build.json")
            } else {
                output.clone()
            };
            maintenance::verify(backend, &receipt)?;
            maintenance::verify_summary(&receipt)
        }
        CloneCommand::Storage(_) => resources::run(command, backend)?,
        CloneCommand::Cache(options) => cache::run(backend, options)?,
        CloneCommand::Runtime(options)
            if options.operation == CloneRuntimeOperation::Status
                && !cache::handoff::selected(backend, options)? =>
        {
            lifecycle::runtime_status::run(backend, options)?
        }
        CloneCommand::Runtime(options) => cache::handoff::run(backend, options)?,
        CloneCommand::PostCopy(options) => post_process::entry::run(backend, options)?,
        CloneCommand::SeedRuntime(options) => seed::run(backend, options)?,
        CloneCommand::Help => return Err("clone 原生模块不处理帮助命令".into()),
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
