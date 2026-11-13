use std::path::Path;

use crate::{
    Result,
    cli::{
        RUNTIME_USAGE, RuntimeBindOptions, RuntimeBuildOptions, RuntimeCommand,
        RuntimeControlInputs, RuntimeGenerationOptions, RuntimeSources, RuntimeStartOptions,
        RuntimeVerifyOptions,
    },
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
};

#[path = "runtime/build.rs"]
pub(in crate::recovery) mod build;

#[path = "runtime/bind.rs"]
mod bind;
#[path = "runtime/business_evidence.rs"]
pub(in crate::recovery) mod business_evidence;
pub(crate) use business_evidence::restore_evidence_paths;
#[path = "runtime/dataset_authority/mod.rs"]
pub(crate) mod dataset_authority;
#[path = "runtime/frontend_site.rs"]
pub(crate) mod frontend_site;
#[path = "runtime/generation/mod.rs"]
mod generation;
#[path = "runtime/live_verify.rs"]
pub(in crate::recovery) mod live_verify;
#[path = "runtime/performance.rs"]
pub(crate) mod performance;
#[path = "runtime/probes.rs"]
mod probes;

#[path = "runtime/recover.rs"]
mod recover;
#[path = "runtime/registration.rs"]
mod registration;
#[path = "runtime/restore_guard.rs"]
pub(in crate::recovery) mod restore_guard;
#[path = "runtime/start.rs"]
pub(crate) mod start;
#[path = "runtime/status.rs"]
mod status;
#[path = "runtime/stop.rs"]
mod stop;
#[path = "runtime/verify.rs"]
mod verify;

const REGISTRATION_GUARD: &str = "restore-runtime-registration.guard";
const REGISTRATION_LOCK: &str = "restore-runtime-registration.lock";

pub(super) fn run(command: &RuntimeCommand, root: &Path, frontend: &Path) -> Result<()> {
    let result = match command {
        RuntimeCommand::Help(operation) => {
            print_help(*operation);
            return Ok(());
        }
        RuntimeCommand::Build(options) => {
            validate_build(options, frontend)?;
            build::run(root, frontend, options)?
        }
        RuntimeCommand::Register(options) => {
            local(&options.plan, root, LocalTestPathKind::ExistingFile)?;
            local(&options.target_plan, root, LocalTestPathKind::ExistingFile)?;
            local(&options.output, root, LocalTestPathKind::OutputFile)?;
            registration::run(root, options)?
        }
        RuntimeCommand::Start(options) => {
            validate_start(options, root)?;
            start::run(root, options)?
        }
        RuntimeCommand::Status(options) => {
            validate_control(options, root)?;
            status::run(root, options)?
        }
        RuntimeCommand::Stop(options) => {
            validate_generation(options, root)?;
            stop::run(root, options)?
        }
        RuntimeCommand::Recover(options) => {
            validate_generation(&options.generation, root)?;
            local(&options.owner, root, LocalTestPathKind::ExistingFile)?;
            recover::run(root, options)?
        }
        RuntimeCommand::Bind(options) => {
            validate_bind(options, root)?;
            bind::run(root, options)?
        }
        RuntimeCommand::Verify(options) => {
            validate_verify(options, root)?;
            verify::run(root, options)?
        }
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

fn print_help(operation: Option<crate::cli::RuntimeOperation>) {
    if let Some(operation) = operation {
        println!("runtime {}\n\n{RUNTIME_USAGE}", operation.as_str());
    } else {
        println!("{RUNTIME_USAGE}");
    }
}

fn validate_build(options: &RuntimeBuildOptions, frontend: &Path) -> Result<()> {
    validate_sources(&options.sources)?;
    external(frontend, LocalTestPathKind::ExistingDirectory)?;
    local(
        &options.output,
        &options.sources.source_backend,
        LocalTestPathKind::OutputFile,
    )?;
    validate_commit_sha(&options.expected_head, "后端恢复构建提交")?;
    validate_commit_sha(&options.expected_frontend_head, "前端恢复构建提交")?;

    Ok(())
}

fn validate_start(options: &RuntimeStartOptions, root: &Path) -> Result<()> {
    validate_control(&options.control, root)?;
    validate_sources(&options.sources)?;
    local(
        &options.build_receipt,
        &options.sources.source_backend,
        LocalTestPathKind::ExistingFile,
    )?;
    local(&options.bindings, root, LocalTestPathKind::ExistingFile)?;
    if options.timeout.is_zero() {
        return Err("恢复运行 timeout 必须是正数".into());
    }
    Ok(())
}

fn validate_bind(options: &RuntimeBindOptions, root: &Path) -> Result<()> {
    validate_sources(&options.sources)?;
    local(
        &options.build_receipt,
        &options.sources.source_backend,
        LocalTestPathKind::ExistingFile,
    )?;
    local(
        &options.launch_receipt,
        root,
        LocalTestPathKind::ExistingFile,
    )?;
    local(&options.bindings, root, LocalTestPathKind::ExistingFile)?;
    local(&options.output, root, LocalTestPathKind::OutputFile)
}

fn validate_verify(options: &RuntimeVerifyOptions, root: &Path) -> Result<()> {
    validate_sources(&options.sources)?;
    local(&options.bindings, root, LocalTestPathKind::ExistingFile)?;
    local(&options.receipt, root, LocalTestPathKind::ExistingFile)
}

fn validate_sources(sources: &RuntimeSources) -> Result<()> {
    external(
        &sources.source_backend,
        LocalTestPathKind::ExistingDirectory,
    )?;
    external(
        &sources.source_frontend,
        LocalTestPathKind::ExistingDirectory,
    )?;
    if let Some(product) = &sources.product_backend {
        external(product, LocalTestPathKind::ExistingDirectory)?;
    }
    match (&sources.adapter_contract, &sources.product_backend) {
        (None, None) => Ok(()),
        (Some(contract), Some(_)) if contract == crate::recovery::b0::CONTRACT => Ok(()),
        (Some(_), Some(_)) => Err("未知的后端适配合同".into()),
        _ => Err("适配合同与产品后端必须成对提供".into()),
    }
}

fn validate_commit_sha(value: &str, label: &str) -> Result<()> {
    if value.len() == 40
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(format!("{label}必须是非零的 40 位小写十六进制 SHA").into())
    }
}

fn validate_control(control: &RuntimeControlInputs, root: &Path) -> Result<()> {
    local(
        &control.runtime_registration,
        root,
        LocalTestPathKind::ExistingFile,
    )?;
    local(&control.target_plan, root, LocalTestPathKind::ExistingFile)
}

fn validate_generation(options: &RuntimeGenerationOptions, root: &Path) -> Result<()> {
    if options.generation == 0 {
        return Err("恢复运行 generation 必须是正整数".into());
    }
    validate_control(&options.control, root)
}

fn local(path: &Path, root: &Path, kind: LocalTestPathKind) -> Result<()> {
    validate_local_test_path(path, root, kind)
        .map(|_| ())
        .map_err(Into::into)
}

fn external(path: &Path, kind: LocalTestPathKind) -> Result<()> {
    validate_absolute_path(path, kind)
        .map(|_| ())
        .map_err(Into::into)
}
