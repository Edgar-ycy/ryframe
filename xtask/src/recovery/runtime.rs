use std::path::Path;

use serde_json::{Map, Value, json};

use crate::{
    Result,
    cli::{
        LEGACY_B0_ADAPTER_CONTRACT, RUNTIME_USAGE, RuntimeBindOptions, RuntimeBuildOptions,
        RuntimeCommand, RuntimeControlInputs, RuntimeGenerationOptions, RuntimeRecoverOptions,
        RuntimeSources, RuntimeStartOptions, RuntimeVerifyOptions,
    },
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    process::run_with_env_removed,
};

use super::{RUNTIME_PROTOCOL_ENV as PROTOCOL_KEY, SOURCE_PROTOCOL_ENV};

const SCRIPT: &str = "scripts/restore_runtime.py";
const MAX_PROTOCOL_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateRuntimeInvocation {
    pub(crate) script: &'static str,
    pub(crate) protocol: String,
}

pub(super) fn run(command: &RuntimeCommand, root: &Path, frontend: &Path) -> Result<()> {
    if let RuntimeCommand::Help(operation) = command {
        print_help(*operation);
        return Ok(());
    }
    let invocation = private_invocation_at(command, root, frontend)?;
    run_with_env_removed(
        root,
        "python",
        &["-B", invocation.script],
        &[(PROTOCOL_KEY, invocation.protocol.as_str())],
        &[PROTOCOL_KEY, SOURCE_PROTOCOL_ENV],
    )
}

fn print_help(operation: Option<crate::cli::RuntimeOperation>) {
    if let Some(operation) = operation {
        println!("runtime {}\n\n{RUNTIME_USAGE}", operation.as_str());
    } else {
        println!("{RUNTIME_USAGE}");
    }
}

pub(crate) fn private_invocation_at(
    command: &RuntimeCommand,
    root: &Path,
    frontend: &Path,
) -> Result<PrivateRuntimeInvocation> {
    if matches!(command, RuntimeCommand::Help(_)) {
        return Err("runtime 帮助不启动私有阶段程序".into());
    }
    validate_command(command, root, frontend)?;
    let operation = command.operation().ok_or("runtime 命令缺少明确操作")?;
    let mut protocol = base_protocol(operation.as_str(), root, command.writes())?;
    match command {
        RuntimeCommand::Build(options) => build_protocol(&mut protocol, options, frontend)?,
        RuntimeCommand::Register(options) => {
            insert_path(&mut protocol, "plan", &options.plan)?;
            insert_path(&mut protocol, "target_plan", &options.target_plan)?;
            insert_path(&mut protocol, "output", &options.output)?;
        }
        RuntimeCommand::Start(options) => start_protocol(&mut protocol, options)?,
        RuntimeCommand::Status(control) => control_protocol(&mut protocol, control)?,
        RuntimeCommand::Stop(options) => generation_protocol(&mut protocol, options)?,
        RuntimeCommand::Recover(options) => recover_protocol(&mut protocol, options)?,
        RuntimeCommand::Bind(options) => bind_protocol(&mut protocol, options)?,
        RuntimeCommand::Verify(options) => verify_protocol(&mut protocol, options)?,
        RuntimeCommand::Help(_) => unreachable!("帮助已在前面拒绝"),
    }
    Ok(PrivateRuntimeInvocation {
        script: SCRIPT,
        protocol: serialize(protocol)?,
    })
}

fn base_protocol(operation: &str, root: &Path, write: bool) -> Result<Map<String, Value>> {
    let Value::Object(value) = json!({
        "backend_dir": path_text(root, "后端目录")?,
        "format_version": 1,
        "kind": "ryframe-xtask-restore-runtime",
        "operation": operation,
        "write": write,
    }) else {
        unreachable!("固定 JSON 对象")
    };
    Ok(value)
}

fn build_protocol(
    protocol: &mut Map<String, Value>,
    options: &RuntimeBuildOptions,
    frontend: &Path,
) -> Result<()> {
    sources_protocol(protocol, &options.sources)?;
    protocol.insert("expected_head".into(), json!(options.expected_head));
    protocol.insert(
        "expected_frontend_head".into(),
        json!(options.expected_frontend_head),
    );
    insert_path(protocol, "frontend_dir", frontend)?;
    insert_path(protocol, "output", &options.output)
}

fn start_protocol(protocol: &mut Map<String, Value>, options: &RuntimeStartOptions) -> Result<()> {
    control_protocol(protocol, &options.control)?;
    sources_protocol(protocol, &options.sources)?;
    insert_path(protocol, "build_receipt", &options.build_receipt)?;
    insert_path(protocol, "bindings", &options.bindings)?;
    protocol.insert("timeout".into(), json!(options.timeout.as_secs_f64()));
    Ok(())
}

fn generation_protocol(
    protocol: &mut Map<String, Value>,
    options: &RuntimeGenerationOptions,
) -> Result<()> {
    control_protocol(protocol, &options.control)?;
    protocol.insert("generation".into(), json!(options.generation));
    Ok(())
}

fn recover_protocol(
    protocol: &mut Map<String, Value>,
    options: &RuntimeRecoverOptions,
) -> Result<()> {
    generation_protocol(protocol, &options.generation)?;
    insert_path(protocol, "owner", &options.owner)
}

fn bind_protocol(protocol: &mut Map<String, Value>, options: &RuntimeBindOptions) -> Result<()> {
    sources_protocol(protocol, &options.sources)?;
    insert_path(protocol, "build_receipt", &options.build_receipt)?;
    insert_path(protocol, "launch_receipt", &options.launch_receipt)?;
    insert_path(protocol, "bindings", &options.bindings)?;
    insert_path(protocol, "output", &options.output)
}

fn verify_protocol(
    protocol: &mut Map<String, Value>,
    options: &RuntimeVerifyOptions,
) -> Result<()> {
    sources_protocol(protocol, &options.sources)?;
    insert_path(protocol, "bindings", &options.bindings)?;
    insert_path(protocol, "receipt", &options.receipt)
}

fn control_protocol(
    protocol: &mut Map<String, Value>,
    control: &RuntimeControlInputs,
) -> Result<()> {
    insert_path(
        protocol,
        "runtime_registration",
        &control.runtime_registration,
    )?;
    insert_path(protocol, "target_plan", &control.target_plan)
}

fn sources_protocol(protocol: &mut Map<String, Value>, sources: &RuntimeSources) -> Result<()> {
    insert_path(protocol, "source_backend", &sources.source_backend)?;
    insert_path(protocol, "source_frontend", &sources.source_frontend)?;
    if let Some(contract) = &sources.adapter_contract {
        protocol.insert("adapter_contract".into(), json!(contract));
    }
    if let Some(product) = &sources.product_backend {
        insert_path(protocol, "product_backend", product)?;
    }
    Ok(())
}

fn validate_command(command: &RuntimeCommand, root: &Path, frontend: &Path) -> Result<()> {
    match command {
        RuntimeCommand::Build(options) => validate_build(options, root, frontend),
        RuntimeCommand::Register(options) => {
            local(&options.plan, root, LocalTestPathKind::ExistingFile)?;
            local(&options.target_plan, root, LocalTestPathKind::ExistingFile)?;
            local(&options.output, root, LocalTestPathKind::OutputFile)
        }
        RuntimeCommand::Start(options) => validate_start(options, root),
        RuntimeCommand::Status(control) => validate_control(control, root),
        RuntimeCommand::Stop(options) => validate_generation(options, root),
        RuntimeCommand::Recover(options) => {
            validate_generation(&options.generation, root)?;
            local(&options.owner, root, LocalTestPathKind::ExistingFile)
        }
        RuntimeCommand::Bind(options) => validate_bind(options, root),
        RuntimeCommand::Verify(options) => validate_verify(options, root),
        RuntimeCommand::Help(_) => Err("runtime 帮助没有可执行请求".into()),
    }
}

fn validate_build(options: &RuntimeBuildOptions, root: &Path, frontend: &Path) -> Result<()> {
    validate_sources(&options.sources)?;
    external(frontend, LocalTestPathKind::ExistingDirectory)?;
    local(
        &options.output,
        &options.sources.source_backend,
        LocalTestPathKind::OutputFile,
    )?;
    validate_commit_sha(&options.expected_head, "后端恢复构建提交")?;
    validate_commit_sha(&options.expected_frontend_head, "前端恢复构建提交")?;
    let _ = root;
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
        (Some(contract), Some(_)) if contract == LEGACY_B0_ADAPTER_CONTRACT => Ok(()),
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

fn insert_path(protocol: &mut Map<String, Value>, name: &str, path: &Path) -> Result<()> {
    protocol.insert(name.to_owned(), json!(path_text(path, name)?));
    Ok(())
}

fn path_text<'a>(path: &'a Path, label: &str) -> Result<&'a str> {
    path.to_str()
        .ok_or_else(|| format!("{label} 必须能表示为 UTF-8").into())
}

fn serialize(protocol: Map<String, Value>) -> Result<String> {
    let value = serde_json::to_string(&Value::Object(protocol))?;
    if value.len() > MAX_PROTOCOL_BYTES || value.contains(['\n', '\r', '\0']) {
        Err("runtime 私有协议过长或包含换行符、NUL".into())
    } else {
        Ok(value)
    }
}
