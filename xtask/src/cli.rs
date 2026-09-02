use std::{collections::BTreeMap, fmt, path::PathBuf};

use crate::{devex, workspace::default_frontend_dir};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cli {
    pub(crate) frontend_dir: PathBuf,
    pub(crate) command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Doctor,
    Check { scope: CheckScope },
    Contract { operation: ContractOperation },
    FeatureMatrix,
    ReleaseVerify(ReleaseOptions),
    Dev { measure_once: bool },
    Verify { scope: CheckScope, full: bool },
    Resource(ResourceCommand),
    ApiSync(ApiSyncCommand),
    Migrate(MigrationCommand),
    Ci(CiCommand),
    Devex(devex::DevexCommand),
    Help(Option<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CiCommand {
    Plan,
    Preflight,
    RustGate,
    ResourceGate,
    Integration,
    ConsumerContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckScope {
    All,
    Backend,
    Frontend,
}

impl CheckScope {
    fn parse(value: &str) -> Result<Self, CliError> {
        match value {
            "all" => Ok(Self::All),
            "backend" => Ok(Self::Backend),
            "frontend" => Ok(Self::Frontend),
            _ => Err(CliError::new("--scope 只允许 all、backend 或 frontend")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContractOperation {
    Check,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseOptions {
    pub(crate) tag: String,
    pub(crate) backend_repository: String,
    pub(crate) backend_commit: String,
    pub(crate) frontend_repository: String,
    pub(crate) frontend_commit: String,
    pub(crate) manifest_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceAction {
    Preview,
    Check,
    Write,
    Explain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResourceTarget {
    Named(String),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceCommand {
    pub(crate) target: ResourceTarget,
    pub(crate) action: ResourceAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApiSyncCommand {
    Candidate,
    Commit(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationOperation {
    Verify,
    Up,
    Status,
}

impl MigrationOperation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Verify => "verify",
            Self::Up => "up",
            Self::Status => "status",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MigrationTarget {
    Control,
    TenantDataAll,
    TenantDataOne(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationScope {
    Control,
    TenantData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MigrationCommand {
    Freeze,
    Run {
        operation: MigrationOperation,
        target: MigrationTarget,
    },
    New {
        scope: MigrationScope,
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliError {
    message: String,
}

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

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
        return Ok(Cli {
            frontend_dir,
            command: Command::Help(args.into_iter().next()),
        });
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(Cli {
            frontend_dir,
            command: Command::Help(Some(command_name)),
        });
    }

    let command = match command_name.as_str() {
        "doctor" => {
            require_empty(&args, "cargo xtask doctor")?;
            Command::Doctor
        }
        "check" => Command::Check {
            scope: parse_scope_options(&args, false)?.0,
        },
        "contract" => Command::Contract {
            operation: parse_contract(&args)?,
        },
        "feature-matrix" => {
            require_empty(&args, "cargo xtask feature-matrix")?;
            Command::FeatureMatrix
        }
        "release-verify" => Command::ReleaseVerify(parse_release(&args)?),
        "dev" => {
            let measure_once = match args.as_slice() {
                [] => false,
                [flag] if flag == "--measure-once" => true,
                _ => {
                    return Err(CliError::new(
                        "用法：cargo dev [--measure-once]；一次性场景由 RYFRAME_DEVEX_SAVE_CASE 选择",
                    ));
                }
            };
            Command::Dev { measure_once }
        }
        "verify" => {
            let (scope, full) = parse_scope_options(&args, true)?;
            Command::Verify { scope, full }
        }
        "resource" => Command::Resource(parse_resource(&args)?),
        "api-sync" => Command::ApiSync(parse_api_sync(&args)?),
        "migrate" => Command::Migrate(parse_migration(&args)?),
        "ci" => Command::Ci(parse_ci(&args)?),
        "devex" => Command::Devex(devex::parse_command(&args).map_err(CliError::new)?),
        _ => return Err(CliError::new(format!("未知命令：{command_name}"))),
    };

    Ok(Cli {
        frontend_dir,
        command,
    })
}

fn parse_ci(args: &[String]) -> Result<CiCommand, CliError> {
    match args {
        [command] if command == "plan" => Ok(CiCommand::Plan),
        [command] if command == "preflight" => Ok(CiCommand::Preflight),
        [command] if command == "rust-gate" => Ok(CiCommand::RustGate),
        [command] if command == "resource-gate" => Ok(CiCommand::ResourceGate),
        [command] if command == "integration" => Ok(CiCommand::Integration),
        [command] if command == "consumer-contract" => Ok(CiCommand::ConsumerContract),
        _ => Err(CliError::new(
            "用法：cargo xtask ci <plan|preflight|rust-gate|resource-gate|integration|consumer-contract>",
        )),
    }
}

fn parse_scope_options(args: &[String], allow_full: bool) -> Result<(CheckScope, bool), CliError> {
    let mut scope = CheckScope::All;
    let mut scope_seen = false;
    let mut full = false;
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
                scope = CheckScope::parse(value)?;
                scope_seen = true;
                index += 2;
            }
            "--full" if allow_full => {
                if full {
                    return Err(CliError::new("--full 不能重复"));
                }
                full = true;
                index += 1;
            }
            unknown => {
                return Err(CliError::new(format!("未知参数：{unknown}")));
            }
        }
    }
    Ok((scope, full))
}

fn parse_contract(args: &[String]) -> Result<ContractOperation, CliError> {
    match args {
        [operation] if operation == "check" => Ok(ContractOperation::Check),
        _ => Err(CliError::new(
            "用法：cargo xtask contract check [--frontend-dir PATH]；写入契约请使用 cargo api-sync",
        )),
    }
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

fn parse_resource(args: &[String]) -> Result<ResourceCommand, CliError> {
    const USAGE: &str =
        "用法：cargo resource <资源名> [--check|--write|--explain] | cargo resource --all --check";
    let (target, action) = match args {
        [all, check] if all == "--all" && check == "--check" => {
            (ResourceTarget::All, ResourceAction::Check)
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

fn parse_api_sync(args: &[String]) -> Result<ApiSyncCommand, CliError> {
    match args {
        [] => Ok(ApiSyncCommand::Candidate),
        [flag, reference]
            if flag == "--commit"
                && !reference.trim().is_empty()
                && !reference.starts_with('-') =>
        {
            Ok(ApiSyncCommand::Commit(reference.clone()))
        }
        _ => Err(CliError::new("用法：cargo api-sync [--commit GIT_REF]")),
    }
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
    if operation == "freeze" {
        require_empty(&args[1..], "cargo migrate freeze")?;
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

fn migration_usage() -> &'static str {
    "用法：cargo migrate <verify|up|status> [control|tenant-data (--all|--target KEY)]\n\
     或：cargo migrate new <control|tenant-data> <迁移名>"
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
        .is_none_or(|value| value.trim().is_empty() || value.starts_with("--"))
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
            .filter(|value| !value.starts_with("--"))
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

pub(crate) fn print_help(topic: Option<&str>) {
    let help = match topic {
        Some("dev") => {
            "cargo dev [--frontend-dir PATH]\n  监听并管理 API、Worker 与 Vite；失败时保留 last-known-good。\n\
             RYFRAME_DEVEX_SAVE_CASE=<case> cargo dev --measure-once\n  只运行一次保存反馈测量；case 支持 config-only、api-only、worker-only、shared-runtime、locales、migration-only、resource-manifest、cancellation。"
        }
        Some("verify") => {
            "cargo verify [--full] [--scope all|backend|frontend] [--frontend-dir PATH]\n  根据前后端 Git 变更执行最小安全检查；--full 执行完整本地门禁。"
        }
        Some("resource") => {
            "cargo resource <资源名> [--check|--write|--explain]\n  预览、检查、写入或解释一个资源；cargo resource --all --check 只读检查全部资源。"
        }
        Some("api-sync") => {
            "cargo api-sync [--commit GIT_REF] [--frontend-dir PATH]\n  同步开发候选契约，或固定指定提交的正式契约。"
        }
        Some("migrate") => migration_usage(),
        Some("ci") => {
            "cargo xtask ci <plan|preflight|rust-gate|resource-gate|integration|consumer-contract>\n  CI 内部稳定入口。"
        }
        Some("devex") => devex::usage(),
        Some("doctor") => "cargo xtask doctor [--frontend-dir PATH]",
        Some("check") => "cargo xtask check [--scope all|backend|frontend] [--frontend-dir PATH]",
        Some("contract") => "cargo xtask contract check [--frontend-dir PATH]",
        Some("feature-matrix") => "cargo xtask feature-matrix",
        Some("release-verify") => {
            "cargo xtask release-verify --tag vMAJOR.MINOR.PATCH \\\n  --backend-repository OWNER/REPO --backend-commit SHA \\\n  --frontend-repository OWNER/REPO --frontend-commit SHA \\\n  --manifest-path PATH [--frontend-dir PATH]"
        }
        Some(unknown) => {
            eprintln!("未知帮助主题：{unknown}");
            general_help()
        }
        None => general_help(),
    };
    println!("{help}");
}

fn general_help() -> &'static str {
    "RyFrame 开发命令\n\n\
日常命令：\n\
  cargo dev\n\
  cargo verify [--full] [--scope all|backend|frontend]\n\
  cargo resource <资源名> [--check|--write|--explain]\n\
  cargo resource --all --check\n\
  cargo api-sync [--commit GIT_REF]\n\
  cargo migrate <verify|up|status> [control|tenant-data (--all|--target KEY)]\n\
  cargo migrate new <control|tenant-data> <迁移名>\n\n\
内部与 CI 命令：\n\
  cargo xtask doctor [--frontend-dir PATH]\n\
  cargo xtask check [--scope all|backend|frontend] [--frontend-dir PATH]\n\
  cargo xtask contract check [--frontend-dir PATH]\n\
  cargo xtask migrate freeze\n\
  cargo xtask ci <plan|preflight|rust-gate|resource-gate|integration|consumer-contract>\n\
  cargo xtask devex run|summarize|compare ...\n\
  cargo xtask feature-matrix\n\
  cargo xtask release-verify ...\n\n\
运行 `cargo <命令> --help` 查看单个日常命令的说明。"
}
