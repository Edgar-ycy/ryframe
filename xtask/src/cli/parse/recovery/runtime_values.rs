use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path};

use super::super::super::model::{CliError, RuntimeOperation};

const NAMED: [&str; 17] = [
    "--plan",
    "--target-plan",
    "--output",
    "--source-backend",
    "--expected-head",
    "--source-frontend",
    "--expected-frontend-head",
    "--adapter-contract",
    "--product-backend",
    "--runtime-registration",
    "--build-receipt",
    "--bindings",
    "--timeout",
    "--generation",
    "--owner",
    "--launch-receipt",
    "--receipt",
];

pub(super) struct ParsedOptions<'a> {
    pub(super) values: BTreeMap<&'static str, &'a str>,
    pub(super) write: bool,
    pub(super) help: bool,
}

pub(super) fn parse_options(args: &[String]) -> Result<ParsedOptions<'_>, CliError> {
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
            return Err(CliError::new(format!("未知 runtime 参数：{option}")));
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

pub(super) fn parse_operation(value: &str) -> Result<RuntimeOperation, CliError> {
    match value {
        "build" => Ok(RuntimeOperation::Build),
        "register" => Ok(RuntimeOperation::Register),
        "start" => Ok(RuntimeOperation::Start),
        "status" => Ok(RuntimeOperation::Status),
        "stop" => Ok(RuntimeOperation::Stop),
        "recover" => Ok(RuntimeOperation::Recover),
        "bind" => Ok(RuntimeOperation::Bind),
        "verify" => Ok(RuntimeOperation::Verify),
        _ => Err(CliError::new(format!(
            "未知 recovery runtime 子操作：{value}"
        ))),
    }
}

pub(super) fn validate_write(operation: RuntimeOperation, write: bool) -> Result<(), CliError> {
    let read_only = matches!(
        operation,
        RuntimeOperation::Status | RuntimeOperation::Verify
    );
    if read_only && write {
        Err(CliError::new(format!(
            "runtime {} 是只读操作，不接受 --write",
            operation.as_str()
        )))
    } else if !read_only && !write {
        Err(CliError::new(format!(
            "runtime {} 会写入证据或控制进程，必须显式传入 --write",
            operation.as_str()
        )))
    } else {
        Ok(())
    }
}

pub(super) fn ensure_only(
    values: &BTreeMap<&str, &str>,
    allowed: &[&str],
    operation: &str,
) -> Result<(), CliError> {
    if let Some(name) = values.keys().find(|name| !allowed.contains(name)) {
        Err(CliError::new(format!(
            "runtime {operation} 不接受参数 {name}"
        )))
    } else {
        Ok(())
    }
}

pub(super) fn required<'a>(
    values: &'a BTreeMap<&str, &'a str>,
    name: &str,
) -> Result<&'a str, CliError> {
    values
        .get(name)
        .copied()
        .ok_or_else(|| CliError::new(format!("runtime 缺少必需参数 {name}")))
}

pub(super) fn coordinator_path(
    value: &str,
    root: &Path,
    kind: LocalTestPathKind,
    label: &str,
) -> Result<PathBuf, CliError> {
    let path = resolve_from(value, root);
    validate_local_test_path(&path, root, kind)
        .map_err(|error| CliError::new(format!("{label}无效：{error}")))
}

pub(super) fn scoped_local_path(
    value: &str,
    current: &Path,
    owner: &Path,
    kind: LocalTestPathKind,
    label: &str,
) -> Result<PathBuf, CliError> {
    let path = resolve_from(value, current);
    validate_local_test_path(&path, owner, kind)
        .map_err(|error| CliError::new(format!("{label}无效：{error}")))
}

pub(super) fn external_directory(value: &str, label: &str) -> Result<PathBuf, CliError> {
    let path = PathBuf::from(value);
    validate_absolute_path(&path, LocalTestPathKind::ExistingDirectory)
        .map_err(|error| CliError::new(format!("{label}无效：{error}")))
}

fn resolve_from(value: &str, root: &Path) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

pub(super) fn commit_sha(value: &str, name: &str) -> Result<String, CliError> {
    if value.len() == 40
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(value.to_owned())
    } else {
        Err(CliError::new(format!(
            "{name} 必须是非零的 40 位小写十六进制 SHA"
        )))
    }
}

pub(super) fn timeout(value: &str) -> Result<Duration, CliError> {
    let parsed = value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| CliError::new("--timeout 必须是有限正数秒"))?;
    Duration::try_from_secs_f64(parsed)
        .ok()
        .filter(|value| !value.is_zero())
        .ok_or_else(|| CliError::new("--timeout 超出可表示的有限正数范围"))
}

fn valid_value(value: &str) -> bool {
    !value.trim().is_empty() && !value.starts_with('-') && !value.contains(['\r', '\n', '\0'])
}
