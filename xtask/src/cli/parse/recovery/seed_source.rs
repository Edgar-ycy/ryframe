use std::{collections::BTreeMap, path::PathBuf};

use crate::{
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    workspace::root_dir,
};

use super::super::super::model::{CliError, SeedSourceOperation, SeedSourceOptions};

const OPERATIONS: [&str; 9] = [
    "source-register",
    "source-rebind",
    "source-generation-start",
    "source-generation-stop",
    "source-generation-status",
    "source-generation-reboot-status",
    "source-generation-recover",
    "source-export",
    "source-export-reconcile",
];

pub(super) fn selects_source_operation(args: &[String]) -> Result<bool, CliError> {
    let values = args
        .windows(2)
        .filter(|pair| pair[0] == "--operation")
        .map(|pair| pair[1].as_str())
        .collect::<Vec<_>>();
    if values.len() > 1 && values.iter().any(|value| OPERATIONS.contains(value)) {
        return Err(CliError::new("--operation 不能重复"));
    }
    Ok(matches!(values.as_slice(), [value] if OPERATIONS.contains(value)))
}

pub(super) fn parse(args: &[String]) -> Result<SeedSourceOptions, CliError> {
    let parsed = parse_options(args)?;
    let operation = parse_operation(required(&parsed.values, "--operation")?)?;
    let root = root_dir();
    let run_dir = path(
        required(&parsed.values, "--run-dir")?,
        "--run-dir",
        LocalTestPathKind::ExistingDirectory,
        &root,
    )?;
    let request = parsed
        .values
        .get("--request")
        .map(|value| path(value, "--request", LocalTestPathKind::ExistingFile, &root))
        .transpose()?;
    if request.is_some() != operation.requires_request() {
        return Err(CliError::new(format!(
            "{} 的 --request 缺失或不适用",
            operation.as_str()
        )));
    }
    if operation.is_read_only() == parsed.write {
        return Err(CliError::new(if operation.is_read_only() {
            "source-generation-status 是只读操作，不接受 --write".to_owned()
        } else {
            format!(
                "{} 会发布阶段证据，必须显式传入 --write",
                operation.as_str()
            )
        }));
    }
    Ok(SeedSourceOptions {
        operation,
        run_dir,
        request,
        write: parsed.write,
    })
}

struct ParsedOptions<'a> {
    values: BTreeMap<&'static str, &'a str>,
    write: bool,
}

fn parse_options(args: &[String]) -> Result<ParsedOptions<'_>, CliError> {
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
        let name = match option {
            "--operation" => "--operation",
            "--run-dir" => "--run-dir",
            "--request" => "--request",
            _ => return Err(CliError::new(format!("未知 seed source 参数：{option}"))),
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

fn required<'a>(values: &'a BTreeMap<&str, &'a str>, name: &str) -> Result<&'a str, CliError> {
    values
        .get(name)
        .copied()
        .ok_or_else(|| CliError::new(format!("seed source 缺少必需参数 {name}")))
}

fn parse_operation(value: &str) -> Result<SeedSourceOperation, CliError> {
    match value {
        "source-register" => Ok(SeedSourceOperation::Register),
        "source-rebind" => Ok(SeedSourceOperation::Rebind),
        "source-generation-start" => Ok(SeedSourceOperation::GenerationStart),
        "source-generation-stop" => Ok(SeedSourceOperation::GenerationStop),
        "source-generation-status" => Ok(SeedSourceOperation::GenerationStatus),
        "source-generation-reboot-status" => Ok(SeedSourceOperation::GenerationRebootStatus),
        "source-generation-recover" => Ok(SeedSourceOperation::GenerationRecover),
        "source-export" => Ok(SeedSourceOperation::Export),
        "source-export-reconcile" => Ok(SeedSourceOperation::ExportReconcile),
        _ => Err(CliError::new(format!("未知 seed source 操作：{value}"))),
    }
}

fn path(
    value: &str,
    label: &str,
    kind: LocalTestPathKind,
    root: &std::path::Path,
) -> Result<PathBuf, CliError> {
    let value = PathBuf::from(value);
    validate_local_test_path(&value, root, kind)
        .map_err(|error| CliError::new(format!("{label} 无效：{error}")))
}

fn valid_value(value: &str) -> bool {
    !value.trim().is_empty() && !value.starts_with('-') && !value.contains(['\r', '\n', '\0'])
}
