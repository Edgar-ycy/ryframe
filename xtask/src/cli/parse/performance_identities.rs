use std::path::PathBuf;

use crate::{
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    workspace::root_dir,
};

use super::{CliError, PerformanceIdentitiesCommand};

const USAGE: &str = "用法：cargo xtask data performance-identities plan --environment <绝对环境清单> --output <绝对计划> --write\n  cargo xtask data performance-identities <apply|verify> --plan <绝对计划> --state-dir <绝对账本目录> --write";

pub(super) fn parse_performance_identities(
    args: &[String],
) -> Result<PerformanceIdentitiesCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new(USAGE));
    };
    let mut environment = None;
    let mut output = None;
    let mut plan = None;
    let mut state_dir = None;
    let mut write = false;
    let mut index = 0;
    while index < values.len() {
        let name = values[index].as_str();
        if name == "--write" {
            if write {
                return Err(CliError::new("--write 不能重复"));
            }
            write = true;
            index += 1;
            continue;
        }
        let target = match name {
            "--environment" => &mut environment,
            "--output" => &mut output,
            "--plan" => &mut plan,
            "--state-dir" => &mut state_dir,
            _ => return Err(CliError::new(format!("未知参数：{name}"))),
        };
        if target.is_some() {
            return Err(CliError::new(format!("{name} 不能重复")));
        }
        let value = values
            .get(index + 1)
            .filter(|value| valid_value(value))
            .ok_or_else(|| CliError::new(format!("{name} 缺少有效路径")))?;
        *target = Some(PathBuf::from(value));
        index += 2;
    }
    if !write {
        return Err(CliError::new("性能身份准备必须显式传入 --write"));
    }
    build_command(operation, environment, output, plan, state_dir)
}

fn build_command(
    operation: &str,
    environment: Option<PathBuf>,
    output: Option<PathBuf>,
    plan: Option<PathBuf>,
    state_dir: Option<PathBuf>,
) -> Result<PerformanceIdentitiesCommand, CliError> {
    let root = root_dir();
    match (operation, environment, output, plan, state_dir) {
        ("plan", Some(environment), Some(output), None, None) => {
            validate(
                &environment,
                &root,
                LocalTestPathKind::ExistingFile,
                "环境清单",
            )?;
            validate(&output, &root, LocalTestPathKind::OutputFile, "身份计划")?;
            Ok(PerformanceIdentitiesCommand::Plan {
                environment,
                output,
            })
        }
        ("apply", None, None, Some(plan), Some(state_dir)) => {
            validate_run("apply", plan, state_dir, &root)
        }
        ("verify", None, None, Some(plan), Some(state_dir)) => {
            validate_run("verify", plan, state_dir, &root)
        }
        ("plan" | "apply" | "verify", ..) => Err(CliError::new(USAGE)),
        _ => Err(CliError::new(format!(
            "未知 performance-identities 子操作：{operation}"
        ))),
    }
}

fn validate_run(
    operation: &str,
    plan: PathBuf,
    state_dir: PathBuf,
    root: &std::path::Path,
) -> Result<PerformanceIdentitiesCommand, CliError> {
    validate(&plan, root, LocalTestPathKind::ExistingFile, "身份计划")?;
    validate(
        &state_dir,
        root,
        LocalTestPathKind::StateDirectory,
        "身份账本目录",
    )?;
    Ok(if operation == "apply" {
        PerformanceIdentitiesCommand::Apply { plan, state_dir }
    } else {
        PerformanceIdentitiesCommand::Verify { plan, state_dir }
    })
}

fn validate(
    value: &std::path::Path,
    root: &std::path::Path,
    kind: LocalTestPathKind,
    label: &str,
) -> Result<(), CliError> {
    validate_local_test_path(value, root, kind)
        .map(|_| ())
        .map_err(|error| CliError::new(format!("{label}无效：{error}")))
}

fn valid_value(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('-') && !value.contains(['\n', '\r', '\0'])
}
