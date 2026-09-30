use std::path::Path;

use serde_json::{Map, Value, json};

use crate::{
    Result,
    cli::{SOURCE_USAGE, SourceCommand, SourceComparisonCaptureOptions, is_source_runtime_output},
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    process::run_with_env_removed,
};

use super::{RUNTIME_PROTOCOL_ENV, SOURCE_PROTOCOL_ENV as PROTOCOL_KEY};

const SCRIPT: &str = "tools/python/restore_source.py";
const MAX_PROTOCOL_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateSourceInvocation {
    pub(crate) script: &'static str,
    pub(crate) protocol: String,
}

pub(super) fn run(command: &SourceCommand, root: &Path) -> Result<()> {
    if let SourceCommand::Help(operation) = command {
        if let Some(operation) = operation {
            println!("source {}\n\n{SOURCE_USAGE}", operation.as_str());
        } else {
            println!("{SOURCE_USAGE}");
        }
        return Ok(());
    }
    let invocation = private_invocation_at(command, root)?;
    run_with_env_removed(
        root,
        "python",
        &["-B", invocation.script],
        &[(PROTOCOL_KEY, invocation.protocol.as_str())],
        &[PROTOCOL_KEY, RUNTIME_PROTOCOL_ENV],
    )
}

pub(crate) fn private_invocation_at(
    command: &SourceCommand,
    root: &Path,
) -> Result<PrivateSourceInvocation> {
    if matches!(command, SourceCommand::Help(_)) {
        return Err("source 帮助不启动私有阶段程序".into());
    }
    validate_command(command, root)?;
    let operation = command.operation().ok_or("source 命令缺少明确操作")?;
    let mut protocol = base_protocol(operation.as_str(), root, command.writes())?;
    match command {
        SourceCommand::Verify(options) => {
            insert_path(
                &mut protocol,
                "source_generation",
                &options.source_generation,
            )?;
            insert_path(&mut protocol, "output", &options.output)?;
        }
        SourceCommand::VerifyRecover(options) => {
            insert_path(
                &mut protocol,
                "source_generation",
                &options.source_generation,
            )?;
            insert_path(&mut protocol, "output", &options.output)?;
        }
        SourceCommand::ComparisonCapture(options) => capture_protocol(&mut protocol, options)?,
        SourceCommand::ComparisonVerify { receipt } => {
            insert_path(&mut protocol, "receipt", receipt)?;
        }
        SourceCommand::Help(_) => unreachable!("帮助已在前面拒绝"),
    }
    Ok(PrivateSourceInvocation {
        script: SCRIPT,
        protocol: serialize(protocol)?,
    })
}

fn base_protocol(operation: &str, root: &Path, write: bool) -> Result<Map<String, Value>> {
    let Value::Object(value) = json!({
        "backend_dir": path_text(root, "后端目录")?,
        "format_version": 1,
        "kind": "ryframe-xtask-restore-source",
        "operation": operation,
        "write": write,
    }) else {
        unreachable!("固定 JSON 对象")
    };
    Ok(value)
}

fn capture_protocol(
    protocol: &mut Map<String, Value>,
    options: &SourceComparisonCaptureOptions,
) -> Result<()> {
    for (name, path) in [
        ("b0_backend", options.b0_backend.as_path()),
        ("b0_adapter_backend", options.b0_adapter_backend.as_path()),
        ("b0_frontend", options.b0_frontend.as_path()),
        ("b0_backend_build", options.b0_backend_build.as_path()),
        ("b0_frontend_build", options.b0_frontend_build.as_path()),
        ("b1_backend", options.b1_backend.as_path()),
        ("b1_frontend", options.b1_frontend.as_path()),
        ("b1_backend_build", options.b1_backend_build.as_path()),
        ("b1_frontend_build", options.b1_frontend_build.as_path()),
        (
            "source_export_result",
            options.source_export_result.as_path(),
        ),
        ("output", options.output.as_path()),
    ] {
        insert_path(protocol, name, path)?;
    }
    Ok(())
}

fn validate_command(command: &SourceCommand, root: &Path) -> Result<()> {
    match command {
        SourceCommand::Verify(options) => {
            local(
                &options.source_generation,
                root,
                LocalTestPathKind::ExistingFile,
            )?;
            local(&options.output, root, LocalTestPathKind::OutputFile)?;
            if is_source_runtime_output(&options.output) {
                Ok(())
            } else {
                Err("来源验证收据必须使用同代 verification/source-runtime.json".into())
            }
        }
        SourceCommand::VerifyRecover(options) => {
            local(
                &options.source_generation,
                root,
                LocalTestPathKind::ExistingFile,
            )?;
            local(&options.output, root, LocalTestPathKind::OutputFile)?;
            local(
                options
                    .output
                    .parent()
                    .ok_or("来源恢复收据缺少 verification 父目录")?,
                root,
                LocalTestPathKind::ExistingDirectory,
            )?;
            if is_source_runtime_output(&options.output) {
                Ok(())
            } else {
                Err("来源恢复收据必须使用同代 verification/source-runtime.json".into())
            }
        }
        SourceCommand::ComparisonCapture(options) => validate_capture(options, root),
        SourceCommand::ComparisonVerify { receipt } => {
            local(receipt, root, LocalTestPathKind::ExistingFile)
        }
        SourceCommand::Help(_) => Err("source 帮助没有可执行请求".into()),
    }
}

fn validate_capture(options: &SourceComparisonCaptureOptions, root: &Path) -> Result<()> {
    let sources = [
        options.b0_backend.as_path(),
        options.b0_adapter_backend.as_path(),
        options.b0_frontend.as_path(),
        options.b1_backend.as_path(),
        options.b1_frontend.as_path(),
    ];
    for source in sources {
        external(source, LocalTestPathKind::ExistingDirectory)?;
    }
    local(
        &options.b0_backend_build,
        &options.b0_adapter_backend,
        LocalTestPathKind::ExistingFile,
    )?;
    local(
        &options.b1_backend_build,
        &options.b1_backend,
        LocalTestPathKind::ExistingFile,
    )?;
    frontend_receipt(&options.b0_frontend_build, &options.b0_frontend)?;
    frontend_receipt(&options.b1_frontend_build, &options.b1_frontend)?;
    local(
        &options.source_export_result,
        root,
        LocalTestPathKind::ExistingFile,
    )?;
    local(&options.output, root, LocalTestPathKind::OutputFile)?;
    distinct_sources(&sources)
}

fn frontend_receipt(receipt: &Path, frontend: &Path) -> Result<()> {
    external(receipt, LocalTestPathKind::ExistingFile)?;
    if receipt == frontend.join("dist/.vite/restore-build.json") {
        Ok(())
    } else {
        Err("前端恢复构建收据必须位于对应来源的 dist/.vite/restore-build.json".into())
    }
}

fn distinct_sources(sources: &[&Path; 5]) -> Result<()> {
    for (index, source) in sources.iter().enumerate() {
        if sources[..index].contains(source) {
            return Err("B0/B1 产品、适配和前端必须使用五个独立工作树".into());
        }
    }
    Ok(())
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
        Err("source 私有协议过长或包含换行符、NUL".into())
    } else {
        Ok(value)
    }
}
