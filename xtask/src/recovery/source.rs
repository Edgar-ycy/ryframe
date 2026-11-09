use std::path::Path;

use crate::{
    Result,
    cli::{SOURCE_USAGE, SourceCommand, SourceComparisonCaptureOptions, is_source_runtime_output},
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
};
#[path = "source/evidence.rs"]
pub(crate) mod evidence;

pub(super) fn run(command: &SourceCommand, root: &Path) -> Result<()> {
    if let SourceCommand::Help(operation) = command {
        if let Some(operation) = operation {
            println!("source {}\n\n{SOURCE_USAGE}", operation.as_str());
        } else {
            println!("{SOURCE_USAGE}");
        }
        return Ok(());
    }
    validate_command(command, root)?;
    let value = match command {
        SourceCommand::Verify(options) => {
            evidence::execution::run(root, &options.source_generation, &options.output, false)?
        }
        SourceCommand::VerifyRecover(options) => {
            evidence::execution::run(root, &options.source_generation, &options.output, true)?
        }
        SourceCommand::ComparisonCapture(options) => {
            evidence::comparison::run_capture(root, options)?
        }
        SourceCommand::ComparisonVerify { receipt } => {
            evidence::comparison::run_verify(root, receipt)?
        }
        SourceCommand::Help(_) => unreachable!("帮助已处理"),
    };
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}
pub(crate) fn validate_command(command: &SourceCommand, root: &Path) -> Result<()> {
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
