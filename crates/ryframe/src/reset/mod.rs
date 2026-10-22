//! 非生产环境全资源重建的双阶段安全入口。

mod connections;
pub mod engine;
pub mod ledger;
pub mod model;
pub mod runtime;
mod state_paths;

use std::{error::Error, fmt};

use engine::execute;
use ledger::LedgerStore;
use model::{ResetManifest, build_manifest, canonical_json, sha256_hex, validate_database_set};
use runtime::ExternalResetRuntime;
use ryframe_config::{AppConfig, ConfigurationArgs, Environment, ResetConfig};

const USAGE: &str = "用法:\n  ryframe-reset plan [--config-dir <目录>]\n  ryframe-reset execute --plan-hash <sha256> --confirm-reset <精确短语> [--config-dir <目录>]\n执行报告区分 completed（本次完成）、reused（复用原完成）与 failed；原锁释放失败或中断须人工核对，不得删除旧账本。";

pub type ResetResult<T> = Result<T, ResetError>;

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    Plan,
    Execute {
        plan_hash: String,
        confirmation: String,
    },
}

#[derive(Debug)]
pub struct ResetError(String);

impl ResetError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ResetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ResetError {}

pub async fn run(args: Vec<String>) -> ResetResult<()> {
    let arguments =
        ConfigurationArgs::parse(args).map_err(|error| ResetError::new(error.to_string()))?;
    let command = parse_command(arguments.arguments)?;
    let environment = Environment::from_required_env()
        .map_err(|_| ResetError::new("APP_ENV 必须显式设置为 dev、test 或 prod"))?;

    // 永久安全边界：必须在读取应用配置、密码、状态文件或连接外部资源之前执行。
    reject_production(environment)?;

    let config = AppConfig::load(&arguments.directory, environment)
        .map_err(|error| ResetError::new(format!("无法加载或校验非生产 reset 配置：{error}")))?;
    let code_sha = load_code_sha(&config.reset)?;
    let manifest = build_manifest(&config, &code_sha)?;
    validate_database_set(&manifest)?;
    let manifest_json = canonical_json(&manifest)?;
    let plan_hash = sha256_hex(&manifest_json);

    match command {
        Command::Plan => {
            println!(
                "{}",
                String::from_utf8(manifest_json).expect("JSON 必须是 UTF-8")
            );
            println!("plan_hash={plan_hash}");
        }
        Command::Execute {
            plan_hash: supplied_hash,
            confirmation,
        } => {
            authorize_execute(&manifest, &plan_hash, &supplied_hash, &confirmation)?;
            require_services_stopped(&config.reset)?;
            let store = LedgerStore::from_config(&config.reset, &manifest, &plan_hash)?;
            let mut runtime = ExternalResetRuntime::new(config, &manifest)?;
            let report = execute(&mut runtime, &manifest, &plan_hash, &store).await?;
            println!("reset_status={}", report.status.as_str());
            if let Some(completed_at) = &report.completed_at {
                println!("reset_completed_at={completed_at}");
            }
            println!("reset_report={}", store.report_path().display());
        }
    }
    Ok(())
}

pub fn parse_command(args: Vec<String>) -> ResetResult<Command> {
    match args.as_slice() {
        [command] if command == "plan" => Ok(Command::Plan),
        [command, hash_flag, plan_hash, confirm_flag, confirmation]
            if command == "execute"
                && hash_flag == "--plan-hash"
                && confirm_flag == "--confirm-reset" =>
        {
            validate_sha256(plan_hash)?;
            Ok(Command::Execute {
                plan_hash: plan_hash.clone(),
                confirmation: confirmation.clone(),
            })
        }
        _ => Err(ResetError::new(USAGE)),
    }
}

pub fn reject_production(environment: Environment) -> ResetResult<()> {
    if environment.is_production() {
        return Err(ResetError::new(
            "生产环境永久禁止运行 ryframe-reset，未读取配置、状态文件或访问外部资源",
        ));
    }
    Ok(())
}

pub fn load_code_sha(config: &ResetConfig) -> ResetResult<String> {
    let value = config.source_sha.as_deref().ok_or_else(|| {
        ResetError::new("所选配置文件缺少 reset.source_sha；请填写 40 位 Git commit SHA")
    })?;
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ResetError::new(
            "所选配置文件 reset.source_sha 必须为 40 位 Git commit SHA；请修正来源字段",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

pub fn authorize_execute(
    manifest: &ResetManifest,
    calculated_hash: &str,
    supplied_hash: &str,
    confirmation: &str,
) -> ResetResult<()> {
    model::validate_manifest_version(manifest)?;
    if supplied_hash != calculated_hash {
        return Err(ResetError::new(
            "plan hash 与当前不可变清单不匹配，请重新运行 plan",
        ));
    }
    if confirmation != manifest.confirmation_phrase {
        return Err(ResetError::new(format!(
            "确认短语不匹配；必须精确传入 {}",
            manifest.confirmation_phrase
        )));
    }
    Ok(())
}

pub fn require_services_stopped(config: &ResetConfig) -> ResetResult<()> {
    if !config.services_stopped {
        return Err(ResetError::new(
            "execute 必须先停止 API、Worker 和 scheduler，并在所选配置文件声明 reset.services_stopped=true",
        ));
    }
    Ok(())
}

pub fn validate_sha256(value: &str) -> ResetResult<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ResetError::new(
            "--plan-hash 必须为 64 位 SHA-256 十六进制字符串",
        ));
    }
    Ok(())
}
