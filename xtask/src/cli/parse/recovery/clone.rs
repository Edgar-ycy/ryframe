use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    workspace::root_dir,
};

use super::super::super::model::{
    CLONE_USAGE, CliError, CloneCommand, CloneRole, CloneSide, CloneStage, CloneStageMode,
};

#[path = "clone/post_seed.rs"]
mod post_seed;
#[path = "clone/services.rs"]
mod services;

use post_seed::{parse_post_copy, parse_seed_runtime};
use services::{parse_cache, parse_runtime, parse_storage};

#[derive(Default)]
struct Options {
    values: BTreeMap<&'static str, String>,
    roles: Option<Vec<CloneRole>>,
    write: bool,
}

pub(super) fn parse(args: &[String]) -> Result<CloneCommand, CliError> {
    let Some((name, rest)) = args.split_first() else {
        return Err(CliError::new(CLONE_USAGE));
    };
    if matches!(name.as_str(), "--help" | "-h") && rest.is_empty() {
        return Ok(CloneCommand::Help);
    }
    if matches!(rest, [help] if matches!(help.as_str(), "--help" | "-h")) {
        return Ok(CloneCommand::Help);
    }
    let command = match name.as_str() {
        "plan" => parse_plan(rest)?,
        "verify" => parse_verify(rest)?,
        "init" => parse_init(rest)?,
        "status" => parse_status(rest)?,
        "stage" => parse_stage(rest)?,
        "runtime" => parse_runtime(rest)?,
        "recover" => parse_recover(rest, false)?,
        "recover-copy" => parse_recover(rest, true)?,
        "bridge" => parse_bridge(rest)?,
        "post-copy" => parse_post_copy(rest)?,
        "seed-runtime" => parse_seed_runtime(rest)?,
        "storage" => parse_storage(rest)?,
        "cache" => parse_cache(rest)?,
        "maintenance" => parse_maintenance(rest)?,
        _ => return Err(CliError::new(format!("未知 recovery clone 子操作：{name}"))),
    };
    Ok(command)
}

fn parse_plan(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(args, &["--input", "--output"], false)?;
    require_write("plan", &options, true)?;
    Ok(CloneCommand::Plan {
        input: path(
            required(&options, "--input")?,
            "--input",
            LocalTestPathKind::ExistingFile,
        )?,
        output: path(
            required(&options, "--output")?,
            "--output",
            LocalTestPathKind::OutputFile,
        )?,
    })
}

fn parse_verify(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(args, &["--plan"], false)?;
    require_write("verify", &options, false)?;
    Ok(CloneCommand::Verify {
        plan: path(
            required(&options, "--plan")?,
            "--plan",
            LocalTestPathKind::ExistingFile,
        )?,
    })
}

fn parse_init(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(args, &["--manifest", "--run-dir"], false)?;
    require_write("init", &options, true)?;
    Ok(CloneCommand::Init {
        manifest: path(
            required(&options, "--manifest")?,
            "--manifest",
            LocalTestPathKind::ExistingFile,
        )?,
        run_dir: path(
            required(&options, "--run-dir")?,
            "--run-dir",
            LocalTestPathKind::NewDirectory,
        )?,
    })
}

fn parse_status(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(args, &["--run-dir"], false)?;
    require_write("status", &options, false)?;
    Ok(CloneCommand::Status {
        run_dir: run_dir(&options)?,
    })
}

fn parse_stage(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(args, &["--run-dir", "--stage", "--mode"], false)?;
    let mode = match optional(&options, "--mode").unwrap_or("run") {
        "run" => CloneStageMode::Run,
        "reconcile" => CloneStageMode::Reconcile,
        "resume" => CloneStageMode::Resume,
        value => return Err(CliError::new(format!("--mode 无效：{value}"))),
    };
    let stage = match required(&options, "--stage")? {
        "export" => CloneStage::Export(mode),
        "target-verify" if mode == CloneStageMode::Run => CloneStage::TargetVerify,
        "target-verify" => {
            return Err(CliError::new(
                "target-verify 只支持首次 run，不接受 reconcile 或 resume",
            ));
        }
        "copy" => CloneStage::Copy(mode),
        value => return Err(CliError::new(format!("--stage 无效：{value}"))),
    };
    require_write("stage", &options, stage.effect().requires_write())?;
    Ok(CloneCommand::Stage {
        run_dir: run_dir(&options)?,
        stage,
    })
}

fn parse_recover(args: &[String], copy: bool) -> Result<CloneCommand, CliError> {
    let options = parse_options(args, &["--run-dir", "--owner-binding"], false)?;
    require_write(
        if copy { "recover-copy" } else { "recover" },
        &options,
        true,
    )?;
    let run_dir = run_dir(&options)?;
    let owner_binding = path(
        required(&options, "--owner-binding")?,
        "--owner-binding",
        LocalTestPathKind::ExistingFile,
    )?;
    Ok(if copy {
        CloneCommand::RecoverCopy {
            run_dir,
            owner_binding,
        }
    } else {
        CloneCommand::Recover {
            run_dir,
            owner_binding,
        }
    })
}

fn parse_bridge(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(args, &["--build", "--inventory", "--output"], false)?;
    require_write("bridge", &options, true)?;
    Ok(CloneCommand::Bridge {
        build: path(
            required(&options, "--build")?,
            "--build",
            LocalTestPathKind::ExistingFile,
        )?,
        inventory: path(
            required(&options, "--inventory")?,
            "--inventory",
            LocalTestPathKind::ExistingFile,
        )?,
        output: path(
            required(&options, "--output")?,
            "--output",
            LocalTestPathKind::OutputFile,
        )?,
    })
}

fn parse_maintenance(args: &[String]) -> Result<CloneCommand, CliError> {
    let options = parse_options(args, &["--operation", "--output"], false)?;
    match required(&options, "--operation")? {
        "build" => {
            require_write("maintenance build", &options, true)?;
            Ok(CloneCommand::MaintenanceBuild {
                output: path(
                    required(&options, "--output")?,
                    "--output",
                    LocalTestPathKind::NewDirectory,
                )?,
            })
        }
        "verify" => {
            require_write("maintenance verify", &options, false)?;
            Ok(CloneCommand::MaintenanceVerify {
                output: existing_path(required(&options, "--output")?, "--output")?,
            })
        }
        value => Err(CliError::new(format!(
            "maintenance --operation 无效：{value}"
        ))),
    }
}

fn parse_options(
    args: &[String],
    named: &[&'static str],
    roles_allowed: bool,
) -> Result<Options, CliError> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        if name == "--write" {
            if options.write {
                return Err(CliError::new("--write 不能重复"));
            }
            options.write = true;
            index += 1;
            continue;
        }
        if name == "--roles" {
            if !roles_allowed {
                return Err(CliError::new("当前 clone 操作不接受 --roles"));
            }
            if options.roles.is_some() {
                return Err(CliError::new("--roles 不能重复"));
            }
            let start = index + 1;
            let mut end = start;
            while end < args.len() && !args[end].starts_with("--") {
                end += 1;
            }
            if start == end {
                return Err(CliError::new("--roles 至少需要 api 或 worker"));
            }
            let mut roles = Vec::new();
            for value in &args[start..end] {
                let role = match value.as_str() {
                    "api" => CloneRole::Api,
                    "worker" => CloneRole::Worker,
                    _ => return Err(CliError::new("--roles 只允许 api 和 worker")),
                };
                if roles.contains(&role) {
                    return Err(CliError::new("--roles 不能包含重复角色"));
                }
                roles.push(role);
            }
            options.roles = Some(roles);
            index = end;
            continue;
        }
        let Some(known) = named.iter().copied().find(|value| *value == name) else {
            return Err(CliError::new(format!("未知 clone 参数：{name}")));
        };
        let value = args
            .get(index + 1)
            .filter(|value| valid_value(value))
            .ok_or_else(|| CliError::new(format!("{known} 缺少有效取值")))?;
        if options.values.insert(known, value.clone()).is_some() {
            return Err(CliError::new(format!("{known} 不能重复")));
        }
        index += 2;
    }
    Ok(options)
}

fn required<'a>(options: &'a Options, name: &str) -> Result<&'a str, CliError> {
    optional(options, name).ok_or_else(|| CliError::new(format!("clone 缺少必需参数 {name}")))
}

fn optional<'a>(options: &'a Options, name: &str) -> Option<&'a str> {
    options.values.get(name).map(String::as_str)
}

fn run_dir(options: &Options) -> Result<PathBuf, CliError> {
    path(
        required(options, "--run-dir")?,
        "--run-dir",
        LocalTestPathKind::ExistingDirectory,
    )
}

fn optional_path(options: &Options, name: &str) -> Result<Option<PathBuf>, CliError> {
    optional(options, name)
        .map(|value| path(value, name, LocalTestPathKind::ExistingFile))
        .transpose()
}

fn path(value: &str, name: &str, kind: LocalTestPathKind) -> Result<PathBuf, CliError> {
    let root = root_dir();
    let candidate = PathBuf::from(value);
    validate_local_test_path(&candidate, &root, kind)
        .map_err(|error| CliError::new(format!("{name} 无效：{error}")))?;
    if matches!(kind, LocalTestPathKind::OutputFile)
        && !candidate.parent().is_some_and(Path::is_dir)
    {
        return Err(CliError::new(format!("{name} 的父目录必须已经存在")));
    }
    Ok(candidate)
}

fn existing_path(value: &str, name: &str) -> Result<PathBuf, CliError> {
    let root = root_dir();
    let candidate = PathBuf::from(value);
    let metadata = fs::symlink_metadata(&candidate)
        .map_err(|error| CliError::new(format!("{name} 不存在或无法核验：{error}")))?;
    let kind = if metadata.is_file() {
        LocalTestPathKind::ExistingFile
    } else if metadata.is_dir() {
        LocalTestPathKind::ExistingDirectory
    } else {
        return Err(CliError::new(format!("{name} 必须是普通文件或目录")));
    };
    validate_local_test_path(&candidate, &root, kind)
        .map_err(|error| CliError::new(format!("{name} 无效：{error}")))
}

fn require_write(name: &str, options: &Options, required: bool) -> Result<(), CliError> {
    if options.write == required {
        Ok(())
    } else if required {
        Err(CliError::new(format!("clone {name} 必须显式传入 --write")))
    } else {
        Err(CliError::new(format!(
            "clone {name} 是只读操作，不接受 --write"
        )))
    }
}

fn side(value: &str) -> Result<CloneSide, CliError> {
    match value {
        "source" => Ok(CloneSide::Source),
        "target" => Ok(CloneSide::Target),
        _ => Err(CliError::new("--side 只允许 source 或 target")),
    }
}

fn valid_value(value: &str) -> bool {
    !value.trim().is_empty() && !value.starts_with('-') && !value.contains(['\r', '\n', '\0'])
}
