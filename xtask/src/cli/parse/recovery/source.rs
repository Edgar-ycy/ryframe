use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::{
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    workspace::root_dir,
};

use super::super::super::model::{
    CliError, SOURCE_USAGE, SourceCommand, SourceComparisonCaptureOptions, SourceOperation,
    SourceVerifyOptions, is_source_runtime_output,
};

const NAMED: [&str; 13] = [
    "--source-generation",
    "--output",
    "--b0-backend",
    "--b0-adapter-backend",
    "--b0-frontend",
    "--b0-backend-build",
    "--b0-frontend-build",
    "--b1-backend",
    "--b1-frontend",
    "--b1-backend-build",
    "--b1-frontend-build",
    "--source-export-result",
    "--receipt",
];

const CAPTURE_NAMED: [&str; 11] = [
    "--output",
    "--b0-backend",
    "--b0-adapter-backend",
    "--b0-frontend",
    "--b0-backend-build",
    "--b0-frontend-build",
    "--b1-backend",
    "--b1-frontend",
    "--b1-backend-build",
    "--b1-frontend-build",
    "--source-export-result",
];

pub(super) fn parse(args: &[String]) -> Result<SourceCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new(SOURCE_USAGE));
    };
    if matches!(operation.as_str(), "--help" | "-h") {
        return if values.is_empty() {
            Ok(SourceCommand::Help(None))
        } else {
            Err(CliError::new(SOURCE_USAGE))
        };
    }
    let operation = parse_operation(operation)?;
    let parsed = parse_options(values)?;
    if parsed.help {
        return Ok(SourceCommand::Help(Some(operation)));
    }
    validate_write(operation, parsed.write)?;
    build_command(operation, &parsed.values)
}

struct ParsedOptions<'a> {
    values: BTreeMap<&'static str, &'a str>,
    write: bool,
    help: bool,
}

fn parse_options(args: &[String]) -> Result<ParsedOptions<'_>, CliError> {
    let mut values = BTreeMap::new();
    let mut write = false;
    let mut help = false;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if matches!(option, "--write" | "--help" | "-h") {
            let target = if option == "--write" {
                &mut write
            } else {
                &mut help
            };
            if *target {
                return Err(CliError::new(format!("{option} 不能重复")));
            }
            *target = true;
            index += 1;
            continue;
        }
        let Some(name) = NAMED.iter().copied().find(|name| *name == option) else {
            return Err(CliError::new(format!("未知 source 参数：{option}")));
        };
        let value = args
            .get(index + 1)
            .map(String::as_str)
            .filter(|value| valid_value(value))
            .ok_or_else(|| CliError::new(format!("{name} 缺少有效取值")))?;
        if values.insert(name, value).is_some() {
            return Err(CliError::new(format!("{name} 不能重复")));
        }
        index += 2;
    }
    Ok(ParsedOptions {
        values,
        write,
        help,
    })
}

fn parse_operation(value: &str) -> Result<SourceOperation, CliError> {
    match value {
        "verify" => Ok(SourceOperation::Verify),
        "verify-recover" => Ok(SourceOperation::VerifyRecover),
        "comparison-capture" => Ok(SourceOperation::ComparisonCapture),
        "comparison-verify" => Ok(SourceOperation::ComparisonVerify),
        _ => Err(CliError::new(format!(
            "未知 recovery source 子操作：{value}"
        ))),
    }
}

fn validate_write(operation: SourceOperation, write: bool) -> Result<(), CliError> {
    let read_only = matches!(operation, SourceOperation::ComparisonVerify);
    if read_only && write {
        Err(CliError::new(
            "source comparison-verify 是只读操作，不接受 --write",
        ))
    } else if !read_only && !write {
        Err(CliError::new(format!(
            "source {} 会发布验证证据，必须显式传入 --write",
            operation.as_str()
        )))
    } else {
        Ok(())
    }
}

fn build_command(
    operation: SourceOperation,
    values: &BTreeMap<&str, &str>,
) -> Result<SourceCommand, CliError> {
    match operation {
        SourceOperation::Verify => verify(values),
        SourceOperation::VerifyRecover => verify_recover(values),
        SourceOperation::ComparisonCapture => comparison_capture(values),
        SourceOperation::ComparisonVerify => comparison_verify(values),
    }
}

fn verify(values: &BTreeMap<&str, &str>) -> Result<SourceCommand, CliError> {
    Ok(SourceCommand::Verify(verify_options(
        values,
        "verify",
        "来源验证收据",
    )?))
}

fn verify_options(
    values: &BTreeMap<&str, &str>,
    operation: &str,
    output_label: &str,
) -> Result<SourceVerifyOptions, CliError> {
    ensure_only(values, &["--source-generation", "--output"], operation)?;
    let root = root_dir();
    let output = local_path(
        required(values, "--output")?,
        &root,
        LocalTestPathKind::OutputFile,
        output_label,
    )?;
    if !is_source_runtime_output(&output) {
        return Err(CliError::new(format!(
            "{output_label}必须使用同代 verification/source-runtime.json"
        )));
    }
    Ok(SourceVerifyOptions {
        source_generation: local_path(
            required(values, "--source-generation")?,
            &root,
            LocalTestPathKind::ExistingFile,
            "来源 generation 收据",
        )?,
        output,
    })
}

fn verify_recover(values: &BTreeMap<&str, &str>) -> Result<SourceCommand, CliError> {
    let options = verify_options(values, "verify-recover", "来源恢复收据")?;
    let parent = options
        .output
        .parent()
        .ok_or_else(|| CliError::new("来源恢复收据缺少 verification 父目录"))?;
    validate_local_test_path(parent, &root_dir(), LocalTestPathKind::ExistingDirectory)
        .map_err(|error| CliError::new(format!("来源恢复现场无效：{error}")))?;
    Ok(SourceCommand::VerifyRecover(options))
}

fn comparison_capture(values: &BTreeMap<&str, &str>) -> Result<SourceCommand, CliError> {
    ensure_only(values, &CAPTURE_NAMED, "comparison-capture")?;
    let root = root_dir();
    let b0_backend = external_directory(required(values, "--b0-backend")?, "B0 产品后端")?;
    let b0_adapter_backend =
        external_directory(required(values, "--b0-adapter-backend")?, "B0 适配后端")?;
    let b0_frontend = external_directory(required(values, "--b0-frontend")?, "B0 前端")?;
    let b1_backend = external_directory(required(values, "--b1-backend")?, "B1 后端")?;
    let b1_frontend = external_directory(required(values, "--b1-frontend")?, "B1 前端")?;
    let options = SourceComparisonCaptureOptions {
        b0_backend,
        b0_backend_build: repository_receipt(
            required(values, "--b0-backend-build")?,
            &root,
            &b0_adapter_backend,
            LocalTestPathKind::ExistingFile,
            "B0 后端构建收据",
        )?,
        b0_frontend_build: frontend_receipt(
            required(values, "--b0-frontend-build")?,
            &b0_frontend,
            "B0 前端构建收据",
        )?,
        b0_adapter_backend,
        b0_frontend,
        b1_backend_build: repository_receipt(
            required(values, "--b1-backend-build")?,
            &root,
            &b1_backend,
            LocalTestPathKind::ExistingFile,
            "B1 后端构建收据",
        )?,
        b1_frontend_build: frontend_receipt(
            required(values, "--b1-frontend-build")?,
            &b1_frontend,
            "B1 前端构建收据",
        )?,
        b1_backend,
        b1_frontend,
        source_export_result: local_path(
            required(values, "--source-export-result")?,
            &root,
            LocalTestPathKind::ExistingFile,
            "来源导出结果",
        )?,
        output: local_path(
            required(values, "--output")?,
            &root,
            LocalTestPathKind::OutputFile,
            "来源比较清单",
        )?,
    };
    distinct_sources(&options)?;
    Ok(SourceCommand::ComparisonCapture(Box::new(options)))
}

fn comparison_verify(values: &BTreeMap<&str, &str>) -> Result<SourceCommand, CliError> {
    ensure_only(values, &["--receipt"], "comparison-verify")?;
    let root = root_dir();
    Ok(SourceCommand::ComparisonVerify {
        receipt: local_path(
            required(values, "--receipt")?,
            &root,
            LocalTestPathKind::ExistingFile,
            "来源比较清单",
        )?,
    })
}

fn ensure_only(
    values: &BTreeMap<&str, &str>,
    allowed: &[&str],
    operation: &str,
) -> Result<(), CliError> {
    if let Some(name) = values.keys().find(|name| !allowed.contains(name)) {
        Err(CliError::new(format!(
            "source {operation} 不接受参数 {name}"
        )))
    } else {
        Ok(())
    }
}

fn required<'a>(values: &'a BTreeMap<&str, &'a str>, name: &str) -> Result<&'a str, CliError> {
    values
        .get(name)
        .copied()
        .ok_or_else(|| CliError::new(format!("source 缺少必需参数 {name}")))
}

fn local_path(
    value: &str,
    root: &Path,
    kind: LocalTestPathKind,
    label: &str,
) -> Result<PathBuf, CliError> {
    let path = resolve_from(value, root);
    validate_local_test_path(&path, root, kind)
        .map_err(|error| CliError::new(format!("{label}无效：{error}")))
}

fn repository_receipt(
    value: &str,
    current: &Path,
    repository: &Path,
    kind: LocalTestPathKind,
    label: &str,
) -> Result<PathBuf, CliError> {
    let path = resolve_from(value, current);
    validate_local_test_path(&path, repository, kind)
        .map_err(|error| CliError::new(format!("{label}无效：{error}")))
}

fn frontend_receipt(value: &str, frontend: &Path, label: &str) -> Result<PathBuf, CliError> {
    let path = PathBuf::from(value);
    validate_absolute_path(&path, LocalTestPathKind::ExistingFile)
        .map_err(|error| CliError::new(format!("{label}无效：{error}")))?;
    if path != frontend.join("dist/.vite/restore-build.json") {
        return Err(CliError::new(format!(
            "{label}必须是对应前端 dist 内的标准恢复构建收据"
        )));
    }
    Ok(path)
}

fn external_directory(value: &str, label: &str) -> Result<PathBuf, CliError> {
    let path = PathBuf::from(value);
    validate_absolute_path(&path, LocalTestPathKind::ExistingDirectory)
        .map_err(|error| CliError::new(format!("{label}无效：{error}")))
}

fn distinct_sources(options: &SourceComparisonCaptureOptions) -> Result<(), CliError> {
    let paths = [
        &options.b0_backend,
        &options.b0_adapter_backend,
        &options.b0_frontend,
        &options.b1_backend,
        &options.b1_frontend,
    ];
    for (index, path) in paths.iter().enumerate() {
        if paths[..index].contains(path) {
            return Err(CliError::new(
                "B0/B1 产品、适配和前端必须使用五个独立工作树",
            ));
        }
    }
    Ok(())
}

fn resolve_from(value: &str, root: &Path) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn valid_value(value: &str) -> bool {
    !value.trim().is_empty() && !value.starts_with('-') && !value.contains(['\r', '\n', '\0'])
}
