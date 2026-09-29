use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use crate::workspace::root_dir;

use super::super::super::model::{
    CliError, FreshTargetCommand, FreshTargetOperation, FreshTargetOptions,
};

pub(super) fn parse(args: &[String]) -> Result<FreshTargetCommand, CliError> {
    if matches!(args, [help] if matches!(help.as_str(), "--help" | "-h")) {
        return Ok(FreshTargetCommand::Help);
    }
    let parsed = parse_options(args)?;
    let operation = parse_operation(required(&parsed.values, "--operation")?)?;
    let options = FreshTargetOptions {
        operation,
        workspace: local_path(required(&parsed.values, "--workspace")?, "--workspace")?,
        request: optional_path(&parsed.values, "--request")?,
        environment: optional_path(&parsed.values, "--environment")?,
        storage_run: optional_path(&parsed.values, "--storage-run")?,
        observation_dir: optional_path(&parsed.values, "--observation-dir")?,
        write: parsed.write,
    };
    validate_shape(&options)?;
    Ok(FreshTargetCommand::Run(options))
}

struct ParsedOptions<'a> {
    values: BTreeMap<&'static str, &'a str>,
    write: bool,
}

fn parse_options(args: &[String]) -> Result<ParsedOptions<'_>, CliError> {
    const NAMED: [&str; 6] = [
        "--workspace",
        "--operation",
        "--request",
        "--environment",
        "--storage-run",
        "--observation-dir",
    ];
    let mut values = BTreeMap::new();
    let mut write = false;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if option == "--write" {
            if write {
                return Err(CliError::new("--write 不能重复"));
            }
            write = true;
            index += 1;
            continue;
        }
        let Some(name) = NAMED.iter().copied().find(|name| *name == option) else {
            return Err(CliError::new(format!("未知 fresh-target 参数：{option}")));
        };
        let value = args
            .get(index + 1)
            .map(String::as_str)
            .filter(|value| valid_value(value))
            .ok_or_else(|| CliError::new(format!("{name} 缺少取值")))?;
        if values.insert(name, value).is_some() {
            return Err(CliError::new(format!("{name} 不能重复")));
        }
        index += 2;
    }
    Ok(ParsedOptions { values, write })
}

fn valid_value(value: &str) -> bool {
    !value.trim().is_empty() && !value.starts_with('-') && !value.contains(['\r', '\n', '\0'])
}

fn required<'a>(values: &'a BTreeMap<&str, &'a str>, name: &str) -> Result<&'a str, CliError> {
    values
        .get(name)
        .copied()
        .ok_or_else(|| CliError::new(format!("fresh-target 缺少必需参数 {name}")))
}

fn optional_path(values: &BTreeMap<&str, &str>, name: &str) -> Result<Option<PathBuf>, CliError> {
    values
        .get(name)
        .map(|value| local_path(value, name))
        .transpose()
}

fn parse_operation(value: &str) -> Result<FreshTargetOperation, CliError> {
    match value {
        "prepare" => Ok(FreshTargetOperation::Prepare),
        "resume-prepare" => Ok(FreshTargetOperation::ResumePrepare),
        "initialize" => Ok(FreshTargetOperation::Initialize),
        "resume-initialize" => Ok(FreshTargetOperation::ResumeInitialize),
        "reconcile-preflight" => Ok(FreshTargetOperation::ReconcilePreflight),
        "verify" => Ok(FreshTargetOperation::Verify),
        "status" => Ok(FreshTargetOperation::Status),
        _ => Err(CliError::new(format!(
            "未知 recovery fresh-target 子操作：{value}"
        ))),
    }
}

fn validate_shape(options: &FreshTargetOptions) -> Result<(), CliError> {
    let actual = (
        options.request.is_some(),
        options.environment.is_some(),
        options.storage_run.is_some(),
        options.observation_dir.is_some(),
        options.write,
    );
    let expected = match options.operation {
        FreshTargetOperation::Prepare => (true, true, true, false, true),
        FreshTargetOperation::Verify => (false, false, false, true, true),
        FreshTargetOperation::Status => (false, false, false, false, false),
        FreshTargetOperation::ResumePrepare
        | FreshTargetOperation::Initialize
        | FreshTargetOperation::ResumeInitialize
        | FreshTargetOperation::ReconcilePreflight => (false, false, false, false, true),
    };
    if actual == expected {
        Ok(())
    } else {
        Err(CliError::new(format!(
            "fresh-target {} 参数或 --write 与阶段不匹配",
            options.operation.as_str()
        )))
    }
}

fn local_path(value: &str, name: &str) -> Result<PathBuf, CliError> {
    let backend = normalize(&root_dir())?;
    let local_tests = backend.join(".local-tests");
    let requested = Path::new(value);
    let candidate = if requested.is_absolute() {
        normalize(requested)?
    } else {
        normalize(&backend.join(requested))?
    };
    if candidate == local_tests || !candidate.starts_with(&local_tests) {
        return Err(CliError::new(format!(
            "{name} 必须位于当前后端 .local-tests 的子目录"
        )));
    }
    reject_existing_links(&backend, &candidate, name)?;
    Ok(candidate)
}

fn normalize(path: &Path) -> Result<PathBuf, CliError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(CliError::new("fresh-target 路径不得包含父目录跳转"));
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    Ok(normalized)
}

fn reject_existing_links(backend: &Path, candidate: &Path, name: &str) -> Result<(), CliError> {
    let relative = candidate
        .strip_prefix(backend)
        .map_err(|_| CliError::new(format!("{name} 越过当前后端目录")))?;
    let mut current = backend.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(CliError::new(format!(
                    "无法核验 {name} 的路径身份：{error}"
                )));
            }
        };
        if is_link_or_reparse(&metadata) {
            return Err(CliError::new(format!(
                "{name} 不能经过符号链接或 Windows reparse point"
            )));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
