use std::{collections::BTreeSet, path::PathBuf};

use super::{CliError, required_argument, valid_git_sha};
use crate::cli::ResourceGateOptions;

pub(super) fn parse(args: &[String]) -> Result<ResourceGateOptions, CliError> {
    let mut options = ResourceGateOptions::default();
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        if !seen.insert(name) {
            return Err(CliError::new(format!("{name} 不能重复")));
        }
        if name == "--targeted" {
            options.targeted = true;
            index += 1;
            continue;
        }
        let value = required_argument(args.get(index + 1), name)?;
        match name {
            "--base-sha" | "--head-sha" | "--frontend-ref" => {
                if !valid_git_sha(value) {
                    return Err(CliError::new(format!(
                        "{name} 必须是非零的 40 位十六进制提交 SHA"
                    )));
                }
                let slot = match name {
                    "--base-sha" => &mut options.base_sha,
                    "--head-sha" => &mut options.head_sha,
                    _ => &mut options.frontend_ref,
                };
                *slot = Some(value.to_ascii_lowercase());
            }
            "--decision-file" => {
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    return Err(CliError::new("--decision-file 必须是绝对路径"));
                }
                options.decision_file = Some(path);
            }
            "--jobs" | "--test-jobs" => {
                let jobs = value
                    .parse::<usize>()
                    .ok()
                    .filter(|jobs| (1..=64).contains(jobs))
                    .ok_or_else(|| CliError::new(format!("{name} 必须是 1 到 64 的整数")))?;
                if name == "--jobs" {
                    options.jobs = Some(jobs);
                } else {
                    options.test_jobs = Some(jobs);
                }
            }
            _ => return Err(CliError::new(format!("未知参数：{name}"))),
        }
        index += 2;
    }
    Ok(options)
}
