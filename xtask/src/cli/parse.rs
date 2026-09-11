use std::{collections::BTreeMap, path::PathBuf};

use crate::{devex, workspace::default_frontend_dir};

use super::model::*;

#[path = "parse/ci.rs"]
mod ci;
#[path = "parse/recovery.rs"]
mod recovery;

use ci::parse_ci;
use recovery::parse_recovery;

pub(crate) fn parse(mut args: Vec<String>) -> Result<Cli, CliError> {
    let frontend_dir = take_unique_option(&mut args, "--frontend-dir")?
        .map(PathBuf::from)
        .unwrap_or_else(default_frontend_dir);
    let Some(command_name) = args.first().cloned() else {
        return Ok(Cli {
            frontend_dir,
            command: Command::Help(None),
        });
    };
    args.remove(0);
    if command_name == "help" || command_name == "--help" || command_name == "-h" {
        if args.len() > 1 {
            return Err(CliError::new("help 最多接收一个命令名称"));
        }
        if let Some(topic) = args.first() {
            require_command_family(topic)?;
        }
        return Ok(Cli {
            frontend_dir,
            command: Command::Help(args.into_iter().next()),
        });
    }
    require_command_family(&command_name)?;
    let recovery_help = command_name == "check"
        && args.first().is_some_and(|arg| arg == "recovery")
        && args.get(1).is_some_and(|arg| !arg.starts_with('-'));
    if !recovery_help && args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(Cli {
            frontend_dir,
            command: Command::Help(Some(command_name)),
        });
    }

    let command = match command_name.as_str() {
        "dev" => parse_dev(&args)?,
        "check" => Command::Check(parse_check(&args)?),
        "build" => Command::Build(parse_build(&args)?),
        "generate" => Command::Generate(parse_generate(&args)?),
        "data" => Command::Data(parse_data(&args)?),
        _ => return Err(CliError::new(format!("未知命令：{command_name}"))),
    };
    Ok(Cli {
        frontend_dir,
        command,
    })
}

fn require_command_family(name: &str) -> Result<(), CliError> {
    match name {
        "dev" | "check" | "build" | "generate" | "data" => Ok(()),
        _ => Err(CliError::new(format!("未知命令：{name}"))),
    }
}

fn parse_dev(args: &[String]) -> Result<Command, CliError> {
    let measure_once = match args {
        [] => false,
        [flag] if flag == "--measure-once" => true,
        _ => {
            return Err(CliError::new(
                "用法：cargo xtask dev [--measure-once]；一次性场景由 RYFRAME_DEVEX_SAVE_CASE 选择",
            ));
        }
    };
    Ok(Command::Dev { measure_once })
}

fn parse_check(args: &[String]) -> Result<CheckCommand, CliError> {
    let Some(first) = args.first() else {
        return Ok(CheckCommand::Run(default_check_options()));
    };
    match first.as_str() {
        "doctor" => {
            require_empty(&args[1..], "cargo xtask check doctor")?;
            Ok(CheckCommand::Doctor)
        }
        "ci" => Ok(CheckCommand::Ci(parse_ci(&args[1..])?)),
        "perf" => Ok(CheckCommand::Perf(
            devex::parse_command(&args[1..]).map_err(CliError::new)?,
        )),
        "release" => Ok(CheckCommand::Release(parse_release(&args[1..])?)),
        "recovery" => Ok(CheckCommand::Recovery(parse_recovery(&args[1..])?)),
        value if value.starts_with('-') => parse_check_options(args).map(CheckCommand::Run),
        _ => Err(CliError::new(format!("未知 check 子任务：{first}"))),
    }
}

fn default_check_options() -> CheckOptions {
    CheckOptions {
        scope: CheckScope::All,
        full: false,
        plan: false,
    }
}

fn parse_check_options(args: &[String]) -> Result<CheckOptions, CliError> {
    let mut options = default_check_options();
    let mut scope_seen = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--scope" => {
                if scope_seen {
                    return Err(CliError::new("--scope 不能重复"));
                }
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CliError::new("--scope 缺少取值"))?;
                options.scope = CheckScope::parse(value)?;
                scope_seen = true;
                index += 2;
            }
            "--full" => {
                set_once(&mut options.full, "--full")?;
                index += 1;
            }
            "--plan" => {
                set_once(&mut options.plan, "--plan")?;
                index += 1;
            }
            unknown => return Err(CliError::new(format!("未知参数：{unknown}"))),
        }
    }
    Ok(options)
}

fn set_once(value: &mut bool, option: &str) -> Result<(), CliError> {
    if *value {
        return Err(CliError::new(format!("{option} 不能重复")));
    }
    *value = true;
    Ok(())
}

fn parse_build(args: &[String]) -> Result<BuildOptions, CliError> {
    let mut profile = BuildProfile::Release;
    let mut profile_seen = false;
    let mut real = false;
    let mut plan = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--profile" => {
                if profile_seen {
                    return Err(CliError::new("--profile 不能重复"));
                }
                profile = match args.get(index + 1).map(String::as_str) {
                    Some("release") => BuildProfile::Release,
                    Some("dev") => BuildProfile::Dev,
                    Some(value) if value.starts_with('-') => {
                        return Err(CliError::new("--profile 缺少取值"));
                    }
                    Some(_) => {
                        return Err(CliError::new("--profile 只允许 release 或 dev"));
                    }
                    None => return Err(CliError::new("--profile 缺少取值")),
                };
                profile_seen = true;
                index += 2;
            }
            "--real" => {
                set_once(&mut real, "--real")?;
                index += 1;
            }
            "--plan" => {
                set_once(&mut plan, "--plan")?;
                index += 1;
            }
            unknown => return Err(CliError::new(format!("未知参数：{unknown}"))),
        }
    }
    Ok(BuildOptions {
        profile,
        real,
        plan,
    })
}

fn parse_generate(args: &[String]) -> Result<GenerateCommand, CliError> {
    match args.split_first() {
        None => Ok(GenerateCommand::Help),
        Some((kind, rest)) if kind == "resource" => {
            Ok(GenerateCommand::Resource(parse_resource(rest)?))
        }
        Some((kind, rest)) if kind == "api" => Ok(GenerateCommand::Api(parse_api(rest)?)),
        Some((kind, _)) => Err(CliError::new(format!("未知 generate 子任务：{kind}"))),
    }
}

fn parse_api(args: &[String]) -> Result<ApiGenerateCommand, CliError> {
    let mut reference = None;
    let mut write = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--write" => {
                set_once(&mut write, "--write")?;
                index += 1;
            }
            "--commit" => {
                if reference.is_some() {
                    return Err(CliError::new("--commit 不能重复"));
                }
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.trim().is_empty() && !value.starts_with('-'))
                    .ok_or_else(|| CliError::new("--commit 缺少 Git 引用"))?;
                reference = Some(value.clone());
                index += 2;
            }
            unknown => return Err(CliError::new(format!("未知参数：{unknown}"))),
        }
    }
    if reference.is_some() && !write {
        return Err(CliError::new("--commit 只用于显式写入，请同时传入 --write"));
    }
    Ok(ApiGenerateCommand { reference, write })
}

fn parse_resource(args: &[String]) -> Result<ResourceCommand, CliError> {
    const USAGE: &str = "用法：cargo xtask generate resource <资源名> [--check|--write|--explain] | cargo xtask generate resource --all <--check|--write>";
    let (target, action) = match args {
        [all, check] if all == "--all" && check == "--check" => {
            (ResourceTarget::All, ResourceAction::Check)
        }
        [all, write] if all == "--all" && write == "--write" => {
            (ResourceTarget::All, ResourceAction::Write)
        }
        [name] if name != "--all" => {
            validate_name(name)?;
            (ResourceTarget::Named(name.clone()), ResourceAction::Preview)
        }
        [name, flag] if name != "--all" => {
            validate_name(name)?;
            let action = match flag.as_str() {
                "--check" => ResourceAction::Check,
                "--write" => ResourceAction::Write,
                "--explain" => ResourceAction::Explain,
                _ => return Err(CliError::new(USAGE)),
            };
            (ResourceTarget::Named(name.clone()), action)
        }
        _ => return Err(CliError::new(USAGE)),
    };
    Ok(ResourceCommand { target, action })
}

fn parse_data(args: &[String]) -> Result<DataCommand, CliError> {
    let Some((kind, rest)) = args.split_first() else {
        return Ok(DataCommand::Help);
    };
    match kind.as_str() {
        "migrate" => Ok(DataCommand::Migrate(parse_migration(rest)?)),
        "backup" => parse_maintenance_args("backup", rest, &["inventory", "register", "status"])
            .map(DataCommand::Backup),
        "restore" => parse_maintenance_args("restore", rest, &["begin", "verify-data", "verify"])
            .map(DataCommand::Restore),
        "target" => {
            parse_maintenance_args("target", rest, &["inventory"]).map(DataCommand::TargetInventory)
        }
        "file" => parse_maintenance_args(
            "file",
            rest,
            &["backfill-sha256", "drain-legacy-reservations"],
        )
        .map(DataCommand::File),
        "reset" => {
            if rest.is_empty() {
                Err(CliError::new("data reset 缺少明确子操作"))
            } else {
                Ok(DataCommand::Reset(rest.to_vec()))
            }
        }
        _ => Err(CliError::new(format!("未知 data 子任务：{kind}"))),
    }
}

fn parse_maintenance_args(
    kind: &str,
    args: &[String],
    operations: &[&str],
) -> Result<Vec<String>, CliError> {
    let Some(operation) = args.first() else {
        return Err(CliError::new(format!("data {kind} 缺少明确子操作")));
    };
    if !operations.contains(&operation.as_str()) {
        return Err(CliError::new(format!(
            "未知 data {kind} 子操作：{operation}"
        )));
    }
    Ok(args.to_vec())
}

fn parse_release(args: &[String]) -> Result<ReleaseOptions, CliError> {
    let values = named_options(
        args,
        &[
            "--tag",
            "--backend-repository",
            "--backend-commit",
            "--frontend-repository",
            "--frontend-commit",
            "--manifest-path",
        ],
    )?;
    Ok(ReleaseOptions {
        tag: required_named(&values, "--tag")?,
        backend_repository: required_named(&values, "--backend-repository")?,
        backend_commit: required_named(&values, "--backend-commit")?,
        frontend_repository: required_named(&values, "--frontend-repository")?,
        frontend_commit: required_named(&values, "--frontend-commit")?,
        manifest_path: required_named(&values, "--manifest-path")?,
    })
}

fn parse_migration(args: &[String]) -> Result<MigrationCommand, CliError> {
    let Some(operation) = args.first() else {
        return Err(CliError::new(migration_usage()));
    };
    if operation == "new" {
        let [_, scope, name] = args else {
            return Err(CliError::new(migration_usage()));
        };
        let scope = match scope.as_str() {
            "control" => MigrationScope::Control,
            "tenant-data" => MigrationScope::TenantData,
            _ => return Err(CliError::new("迁移 scope 只允许 control 或 tenant-data")),
        };
        validate_migration_name(name)?;
        return Ok(MigrationCommand::New {
            scope,
            name: name.clone(),
        });
    }
    if operation == "baseline" {
        if args[1..] != ["--write"] {
            return Err(CliError::new(
                "用法：cargo xtask data migrate baseline --write",
            ));
        }
        return Ok(MigrationCommand::Baseline);
    }
    if operation == "freeze" {
        require_empty(&args[1..], "cargo xtask data migrate freeze")?;
        return Ok(MigrationCommand::Freeze);
    }
    let operation = match operation.as_str() {
        "verify" => MigrationOperation::Verify,
        "up" => MigrationOperation::Up,
        "status" => MigrationOperation::Status,
        _ => return Err(CliError::new(migration_usage())),
    };
    let target = match &args[1..] {
        [] => MigrationTarget::Control,
        [scope] if scope == "control" => MigrationTarget::Control,
        [scope, flag] if scope == "tenant-data" && flag == "--all" => {
            MigrationTarget::TenantDataAll
        }
        [scope, flag, target]
            if scope == "tenant-data"
                && flag == "--target"
                && !target.trim().is_empty()
                && !target.starts_with('-') =>
        {
            MigrationTarget::TenantDataOne(target.clone())
        }
        _ => return Err(CliError::new(migration_usage())),
    };
    Ok(MigrationCommand::Run { operation, target })
}

pub(super) fn migration_usage() -> &'static str {
    "cargo xtask data migrate <verify|up|status> [control|tenant-data (--all|--target KEY)]\n  cargo xtask data migrate new <control|tenant-data> <迁移名>\n  cargo xtask data migrate baseline --write | freeze"
}

fn validate_name(name: &str) -> Result<(), CliError> {
    let mut characters = name.chars();
    let valid_start = characters
        .next()
        .is_some_and(|character| character.is_ascii_lowercase());
    let valid_rest = characters.all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-'
    });
    if valid_start && valid_rest {
        Ok(())
    } else {
        Err(CliError::new(
            "名称必须以小写英文字母开头，且只包含小写字母、数字、下划线或连字符",
        ))
    }
}

fn validate_migration_name(name: &str) -> Result<(), CliError> {
    let mut characters = name.chars();
    let valid_start = characters
        .next()
        .is_some_and(|character| character.is_ascii_lowercase());
    let valid_rest = characters.all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
    });
    if valid_start && valid_rest {
        Ok(())
    } else {
        Err(CliError::new(
            "迁移名必须是 snake_case，且以小写英文字母开头",
        ))
    }
}

fn take_unique_option(args: &mut Vec<String>, option: &str) -> Result<Option<String>, CliError> {
    let positions = args
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| (arg == option).then_some(index))
        .collect::<Vec<_>>();
    if positions.len() > 1 {
        return Err(CliError::new(format!("{option} 不能重复")));
    }
    let Some(position) = positions.first().copied() else {
        return Ok(None);
    };
    if args
        .get(position + 1)
        .is_none_or(|value| value.trim().is_empty() || value.starts_with('-'))
    {
        return Err(CliError::new(format!("{option} 缺少取值")));
    }
    args.remove(position);
    Ok(Some(args.remove(position)))
}

fn named_options(args: &[String], allowed: &[&str]) -> Result<BTreeMap<String, String>, CliError> {
    let mut values = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if !allowed.contains(&option) {
            return Err(CliError::new(format!("未知参数：{option}")));
        }
        let value = args
            .get(index + 1)
            .filter(|value| !value.trim().is_empty() && !value.starts_with('-'))
            .ok_or_else(|| CliError::new(format!("{option} 缺少取值")))?;
        if values.insert(option.to_owned(), value.clone()).is_some() {
            return Err(CliError::new(format!("{option} 不能重复")));
        }
        index += 2;
    }
    Ok(values)
}

fn required_named(values: &BTreeMap<String, String>, option: &str) -> Result<String, CliError> {
    values
        .get(option)
        .cloned()
        .ok_or_else(|| CliError::new(format!("缺少必需参数 {option}")))
}

fn require_empty(args: &[String], usage: &str) -> Result<(), CliError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(CliError::new(format!("用法：{usage}")))
    }
}
