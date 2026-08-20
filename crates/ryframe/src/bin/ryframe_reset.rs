//! 非生产环境全资源重建的双阶段安全入口。

#[path = "ryframe_reset/engine.rs"]
mod engine;
#[path = "ryframe_reset/ledger.rs"]
mod ledger;
#[path = "ryframe_reset/model.rs"]
mod model;
#[path = "ryframe_reset/runtime/mod.rs"]
mod runtime;

use std::{error::Error, fmt};

use engine::execute;
use ledger::LedgerStore;
use model::{ResetManifest, build_manifest, canonical_json, sha256_hex, validate_database_set};
use runtime::ExternalResetRuntime;
use ryframe_config::{AppConfig, Environment};

const USAGE: &str = "用法:\n  ryframe-reset plan\n  ryframe-reset execute --plan-hash <sha256> --confirm-reset <精确短语>";
const CODE_SHA_ENV: &str = "RYFRAME_CODE_SHA";
const SERVICES_STOPPED_ENV: &str = "RYFRAME_RESET_SERVICES_STOPPED";

type ResetResult<T> = Result<T, ResetError>;

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Plan,
    Execute {
        plan_hash: String,
        confirmation: String,
    },
}

#[derive(Debug)]
struct ResetError(String);

impl ResetError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ResetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ResetError {}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let command = parse_command(std::env::args().skip(1).collect())?;
    let environment = Environment::from_required_env()
        .map_err(|_| ResetError::new("APP_ENV 必须显式设置为 dev、test 或 prod"))?;

    // 永久安全边界：必须在读取应用配置、密码、状态文件或连接外部资源之前执行。
    reject_production(environment)?;

    let config = AppConfig::load_from_env(environment)
        .map_err(|_| ResetError::new("无法加载或校验非生产 reset 配置"))?;
    let code_sha = load_code_sha()?;
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
            require_services_stopped()?;
            let store = LedgerStore::from_environment(&manifest, &plan_hash)?;
            let mut runtime = ExternalResetRuntime::new(config, &manifest)?;
            let report = execute(&mut runtime, &manifest, &plan_hash, &store).await?;
            println!("reset_status={}", report.status);
            println!("reset_report={}", store.report_path().display());
        }
    }
    Ok(())
}

fn parse_command(args: Vec<String>) -> ResetResult<Command> {
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

fn reject_production(environment: Environment) -> ResetResult<()> {
    if environment.is_production() {
        return Err(ResetError::new(
            "生产环境永久禁止运行 ryframe-reset，未读取配置、状态文件或访问外部资源",
        ));
    }
    Ok(())
}

fn load_code_sha() -> ResetResult<String> {
    let value = std::env::var(CODE_SHA_ENV)
        .or_else(|_| {
            option_env!("RYFRAME_CODE_SHA")
                .map(str::to_owned)
                .ok_or(std::env::VarError::NotPresent)
        })
        .map_err(|_| ResetError::new(format!("{CODE_SHA_ENV} 必须提供 40 位 Git commit SHA")))?;
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ResetError::new(format!(
            "{CODE_SHA_ENV} 必须提供 40 位 Git commit SHA"
        )));
    }
    Ok(value.to_ascii_lowercase())
}

fn authorize_execute(
    manifest: &ResetManifest,
    calculated_hash: &str,
    supplied_hash: &str,
    confirmation: &str,
) -> ResetResult<()> {
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

fn require_services_stopped() -> ResetResult<()> {
    if std::env::var(SERVICES_STOPPED_ENV).as_deref() != Ok("YES") {
        return Err(ResetError::new(format!(
            "必须先停止 API、Worker 和 scheduler，并设置 {SERVICES_STOPPED_ENV}=YES"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> ResetResult<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ResetError::new(
            "--plan-hash 必须为 64 位 SHA-256 十六进制字符串",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{LegacyOwnershipPolicy, ObjectStorageResource};

    #[test]
    fn command_parser_requires_exact_two_phase_shape() {
        assert_eq!(
            parse_command(vec!["plan".into()]).expect("计划命令有效"),
            Command::Plan
        );
        assert!(parse_command(vec!["execute".into()]).is_err());
        assert!(
            parse_command(vec![
                "execute".into(),
                "--plan-hash".into(),
                "0".repeat(63),
                "--confirm-reset".into(),
                "RESET".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn production_is_rejected_before_manifest_work() {
        assert!(reject_production(Environment::Prod).is_err());
        assert!(reject_production(Environment::Dev).is_ok());
        assert!(reject_production(Environment::Test).is_ok());
    }

    #[test]
    fn production_guard_precedes_configuration_and_external_runtime() {
        let source = include_str!("ryframe_reset.rs");
        let main_body = source
            .split_once("async fn main")
            .and_then(|(_, suffix)| suffix.split_once("fn parse_command"))
            .map(|(body, _)| body)
            .expect("定位 reset main 函数");
        let guard = main_body
            .find("reject_production(environment)")
            .expect("生产环境保护存在");
        let config = main_body
            .find("AppConfig::load_from_env")
            .expect("配置加载存在");
        let runtime = main_body
            .find("ExternalResetRuntime::new")
            .expect("外部资源运行时存在");
        assert!(guard < config);
        assert!(guard < runtime);
    }

    #[test]
    fn redis_reset_source_excludes_broad_deletion_commands() {
        let source = include_str!("ryframe_reset/runtime/resources.rs");
        let forbidden = [
            ["FLUSH", "DB"].concat(),
            ["FLUSH", "ALL"].concat(),
            format!("redis::cmd({:?})", "KEYS"),
        ];
        for command in forbidden {
            assert!(!source.contains(&command), "禁止使用 Redis 广域删除命令");
        }
        assert!(source.contains("redis::cmd(\"SCAN\")"));
        assert!(source.contains("redis::cmd(\"UNLINK\")"));
        assert!(source.contains(".ryframe-reset-probe:"));
        assert!(source.contains("pub async fn prove_capabilities"));
        assert!(source.contains("raw_unlink_exact"));
    }

    #[test]
    fn execution_authorization_is_fail_closed() {
        let manifest = ResetManifest {
            manifest_version: 3,
            environment: "test".into(),
            scope_id: "test-a".into(),
            code_sha: "a".repeat(40),
            config_sha: "b".repeat(64),
            credential_version: "test-v1".into(),
            confirmation_phrase: "RESET-RYFRAME-test-test-a".into(),
            legacy_ownership: LegacyOwnershipPolicy {
                mysql_exclusive: false,
                redis_exclusive: false,
                object_storage_exclusive: false,
            },
            redis: None,
            object_storage: ObjectStorageResource {
                backend: "local".into(),
                endpoint: "unused".into(),
                use_ssl: false,
                region: String::new(),
                access_key_env: None,
                secret_key_env: None,
                prefixes: Vec::new(),
            },
            databases: Vec::new(),
        };
        let hash = "c".repeat(64);
        assert!(authorize_execute(&manifest, &hash, &hash, "wrong").is_err());
        assert!(authorize_execute(&manifest, &hash, &hash, "RESET-RYFRAME-test-test-a").is_ok());
    }
}
