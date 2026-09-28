use std::{collections::BTreeMap, path::PathBuf};

use crate::{
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    workspace::root_dir,
};

use super::super::super::model::{
    CliError, DatasetPrepareCommand, DatasetPrepareMode, DatasetPrepareOptions, RecoverySide,
};

const VALUE_OPTIONS: [&str; 4] = ["--plan", "--preflight", "--verify-existing", "--side"];

pub(super) fn parse(args: &[String]) -> Result<DatasetPrepareCommand, CliError> {
    if matches!(args, [flag] if matches!(flag.as_str(), "--help" | "-h")) {
        return Ok(DatasetPrepareCommand::Help);
    }
    if args
        .iter()
        .any(|value| matches!(value.as_str(), "--help" | "-h"))
    {
        return Err(CliError::new("dataset-prepare 帮助不能与执行参数同时使用"));
    }
    let parsed = parse_options(args)?;
    if !parsed.write {
        return Err(CliError::new("数据准备和认证验收必须显式传入 --write"));
    }
    let root = root_dir();
    let plan = local_file(required(&parsed.values, "--plan")?, "--plan", &root)?;
    let preflight = optional_local_file(&parsed.values, "--preflight", &root)?;
    let dataset = optional_local_file(&parsed.values, "--verify-existing", &root)?;
    let mode = match (preflight, dataset) {
        (Some(preflight), None) => {
            if parsed.values.contains_key("--side") {
                return Err(CliError::new(
                    "--side 仅用于已有数据检查，数据准备固定 source",
                ));
            }
            DatasetPrepareMode::Prepare { preflight }
        }
        (None, Some(dataset)) => DatasetPrepareMode::VerifyExisting {
            dataset,
            side: parse_side(parsed.values.get("--side").copied())?,
        },
        (Some(_), Some(_)) => {
            return Err(CliError::new("数据准备预检与已有数据收据不能同时使用"));
        }
        (None, None) => {
            return Err(CliError::new("必须明确 --preflight 或 --verify-existing"));
        }
    };
    Ok(DatasetPrepareCommand::Run(DatasetPrepareOptions {
        plan,
        mode,
    }))
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
        let Some(canonical) = VALUE_OPTIONS.iter().find(|known| **known == option) else {
            return Err(CliError::new(format!(
                "未知 dataset-prepare 参数：{option}"
            )));
        };
        let value = args
            .get(index + 1)
            .filter(|value| !value.trim().is_empty() && !value.starts_with('-'))
            .ok_or_else(|| CliError::new(format!("{option} 缺少取值")))?;
        if values.insert(*canonical, value.as_str()).is_some() {
            return Err(CliError::new(format!("{option} 不能重复")));
        }
        index += 2;
    }
    Ok(ParsedOptions { values, write })
}

fn required<'a>(values: &BTreeMap<&str, &'a str>, name: &str) -> Result<&'a str, CliError> {
    values
        .get(name)
        .copied()
        .ok_or_else(|| CliError::new(format!("dataset-prepare 缺少必需参数 {name}")))
}

fn optional_local_file(
    values: &BTreeMap<&str, &str>,
    name: &str,
    root: &std::path::Path,
) -> Result<Option<PathBuf>, CliError> {
    values
        .get(name)
        .map(|value| local_file(value, name, root))
        .transpose()
}

fn local_file(value: &str, name: &str, root: &std::path::Path) -> Result<PathBuf, CliError> {
    let path = PathBuf::from(value);
    validate_local_test_path(&path, root, LocalTestPathKind::ExistingFile)
        .map_err(|error| CliError::new(format!("{name} 无效：{error}")))
}

fn parse_side(value: Option<&str>) -> Result<RecoverySide, CliError> {
    match value.unwrap_or("target") {
        "source" => Ok(RecoverySide::Source),
        "target" => Ok(RecoverySide::Target),
        _ => Err(CliError::new("--side 只允许 source 或 target")),
    }
}
