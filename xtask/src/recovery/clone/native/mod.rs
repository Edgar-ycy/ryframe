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
pub(in crate::recovery::clone::native) mod held_plan;
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
        CloneCommand::ObservePrepareTargetLock {
            run_dir,
            owner_binding,
            output,
        } => lifecycle::prepare_target_lock::observe(backend, run_dir, owner_binding, output)?,
        CloneCommand::RecoverPrepareTargetLock { run_dir, receipt } => {
            lifecycle::prepare_target_lock::recover(backend, run_dir, receipt)?
        }
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
        CloneCommand::Runtime(options)
            if options.operation == CloneRuntimeOperation::Recover
                && !cache::handoff::selected(backend, options)? =>
        {
            lifecycle::runtime_recover::run(backend, options)?
        }
        CloneCommand::Runtime(options)
            if options.operation == CloneRuntimeOperation::Stop
                && !cache::handoff::selected(backend, options)? =>
        {
            lifecycle::runtime_stop::run(backend, options)?
        }
        CloneCommand::Runtime(options)
            if options.operation == CloneRuntimeOperation::Start
                && !cache::handoff::selected(backend, options)? =>
        {
            lifecycle::runtime_start::run(backend, options)?
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
    validate_local_test_path(output, backend, LocalTestPathKind::OutputFile)
        .map_err(|error| format!("复制计划输出无效：{error}"))?;
    let inspected = inspect_input(backend, input)?;
    let parent = output.parent().ok_or("复制计划输出缺少父目录")?;
    if std::fs::canonicalize(parent)?.starts_with(std::fs::canonicalize(&inspected.artifact_root)?)
    {
        return Err("计划输出不得写入来源导出目录".into());
    }
    json::write_new(output, &inspected.wrapper)?;
    finish_new_plan(backend, input, output, &inspected)?;
    summary(&inspected.wrapper)
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

pub(crate) struct InspectedInput {
    pub(crate) wrapper: Value,
    declaration: Value,
    input_digest: digest::FileDigest,
    artifact_root: PathBuf,
    source_policy: plan::SourcePolicy,
}

pub(crate) fn inspect_input(backend: &Path, input: &Path) -> Result<InspectedInput> {
    inspect_input_with_objects(backend, input, None)
}

fn inspect_input_with_objects(
    backend: &Path,
    input: &Path,
    held: Option<&held_plan::HeldPlanObjects>,
) -> Result<InspectedInput> {
    validate_local_test_path(input, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("复制输入无效：{error}"))?;
    let before = digest::file(input)?;
    let declaration = json::read(input)?;
    let output = plan::create(&declaration, backend, held)?;
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
    Ok(InspectedInput {
        wrapper,
        declaration,
        input_digest: before,
        artifact_root: output.artifact_root,
        source_policy: output.source_policy,
    })
}

pub(crate) fn finish_new_plan(
    backend: &Path,
    input: &Path,
    path: &Path,
    inspected: &InspectedInput,
) -> Result<()> {
    let plan_before = verify_published_plan(path, &inspected.wrapper)?;
    validate_local_test_path(input, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("复制输入无效：{error}"))?;
    if Some(digest::path_text(input, "复制输入")?.as_str())
        != inspected.wrapper["input_path"].as_str()
        || digest::file(input)? != inspected.input_digest
    {
        return Err("离线核验期间输入清单变化".into());
    }
    plan::recheck_inputs(
        &inspected.declaration,
        backend,
        &inspected.wrapper["plan"]["catalog_sha256"],
        &inspected.source_policy,
    )?;
    if digest::file(input)? != inspected.input_digest {
        return Err("离线核验期间输入清单变化".into());
    }
    verify_plan_digest(path, &plan_before)
}

pub(crate) fn issue(backend: &Path, plan: &Path, wrapper: &Value) -> Result<Value> {
    issue_with_objects(backend, plan, wrapper, None)
}

fn issue_with_objects(
    backend: &Path,
    plan: &Path,
    wrapper: &Value,
    held: Option<&held_plan::HeldPlanObjects>,
) -> Result<Value> {
    let before = verify_published_plan(plan, wrapper)?;
    let input = wrapper["input_path"]
        .as_str()
        .ok_or("计划缺少完整输入路径")?;
    if inspect_input_with_objects(backend, Path::new(input), held)?.wrapper != *wrapper {
        return Err("已写计划与当前完整输入、源码策略或导出证据不一致".into());
    }
    verify_plan_digest(plan, &before)?;
    Ok(wrapper.clone())
}

fn verify_published_plan(path: &Path, wrapper: &Value) -> Result<digest::FileDigest> {
    let before = digest::file(path)?;
    if json::read(path)? != *wrapper {
        return Err("完整验证结果签发前计划文件变化".into());
    }
    Ok(before)
}

fn verify_plan_digest(path: &Path, before: &digest::FileDigest) -> Result<()> {
    if digest::file(path)? != *before {
        return Err("完整验证结果签发前计划文件变化".into());
    }
    Ok(())
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
