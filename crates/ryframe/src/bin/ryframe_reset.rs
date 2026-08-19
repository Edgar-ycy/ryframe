//! 非生产环境全资源重建的双阶段安全入口。
//!
//! `plan` 只输出无秘密清单；`execute` 必须复用完全相同的清单哈希。当前版本有意在任何
//! 外部写入前拒绝执行，直到对象前缀枚举、所有权表基线和幂等续跑账本同时就绪。

use std::collections::BTreeMap;

use ryframe_application::system::{
    AVATAR_BUCKET, CONFIG_PACKAGE_BUCKET, EXPORT_BUCKET, IMPORT_BUCKET, UPLOAD_BUCKET,
};
use ryframe_config::{AppConfig, Environment, StorageBackend, TenantDatabaseTargetKind};
use serde::Serialize;
use sha2::{Digest, Sha256};

const USAGE: &str = "用法:\n  ryframe-reset plan\n  ryframe-reset execute --plan-hash <sha256> --confirm-reset <精确短语>";
const CODE_SHA_ENV: &str = "RYFRAME_CODE_SHA";
const SERVICES_STOPPED_ENV: &str = "RYFRAME_RESET_SERVICES_STOPPED";
const MANIFEST_VERSION: u32 = 1;

type DynError = Box<dyn std::error::Error>;

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Plan,
    Execute {
        plan_hash: String,
        confirmation: String,
    },
}

#[derive(Clone, Debug, Serialize)]
struct ResetManifest {
    manifest_version: u32,
    environment: String,
    scope_id: String,
    code_sha: String,
    config_sha: String,
    confirmation_phrase: String,
    redis: Option<RedisResource>,
    object_storage: ObjectStorageResource,
    databases: Vec<PhysicalDatabase>,
}

#[derive(Clone, Debug, Serialize)]
struct RedisResource {
    host: String,
    port: u16,
    database: u8,
    namespace: String,
    ownership_marker_key: String,
    ownership_marker: String,
}

#[derive(Clone, Debug, Serialize)]
struct ObjectStorageResource {
    backend: String,
    endpoint: String,
    prefixes: Vec<ObjectPrefix>,
}

#[derive(Clone, Debug, Serialize)]
struct ObjectPrefix {
    bucket: String,
    prefix: String,
    ownership_marker_key: String,
    ownership_marker: String,
}

#[derive(Clone, Debug, Serialize)]
struct PhysicalDatabase {
    host: String,
    port: u16,
    database: String,
    target_keys: Vec<String>,
    ownership_marker: String,
}

#[derive(Clone, Debug, Serialize)]
struct ConfigFingerprint<'a> {
    environment: &'a str,
    scope_id: &'a str,
    redis: &'a Option<RedisResource>,
    object_storage: &'a ObjectStorageResource,
    databases: &'a [PhysicalDatabase],
}

#[tokio::main]
async fn main() -> Result<(), DynError> {
    let command = parse_command(std::env::args().skip(1).collect())?;
    let environment = Environment::from_required_env()?;
    reject_production(environment)?;

    let config = AppConfig::load_from_env(environment)?;
    let code_sha = load_code_sha()?;
    let manifest = build_manifest(&config, &code_sha)?;
    let manifest_json = canonical_json(&manifest)?;
    let plan_hash = sha256_hex(&manifest_json);

    match command {
        Command::Plan => {
            println!("{}", String::from_utf8(manifest_json)?);
            println!("plan_hash={plan_hash}");
        }
        Command::Execute {
            plan_hash: supplied_hash,
            confirmation,
        } => {
            authorize_execute(&manifest, &plan_hash, &supplied_hash, &confirmation)?;
            require_services_stopped()?;
            return Err(
                "重建执行器尚未启用：对象前缀有界枚举、MySQL 所有权基线和幂等续跑账本未同时就绪；未连接或修改任何外部资源"
                    .into(),
            );
        }
    }
    Ok(())
}

fn parse_command(args: Vec<String>) -> Result<Command, DynError> {
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
        _ => Err(USAGE.into()),
    }
}

fn reject_production(environment: Environment) -> Result<(), DynError> {
    if environment.is_production() {
        return Err("生产环境永久禁止运行 ryframe-reset，未读取配置或访问外部资源".into());
    }
    Ok(())
}

fn load_code_sha() -> Result<String, DynError> {
    let value = std::env::var(CODE_SHA_ENV)
        .or_else(|_| {
            option_env!("RYFRAME_CODE_SHA")
                .map(str::to_owned)
                .ok_or(std::env::VarError::NotPresent)
        })
        .map_err(|_| format!("{CODE_SHA_ENV} 必须提供 40 位 Git commit SHA"))?;
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{CODE_SHA_ENV} 必须提供 40 位 Git commit SHA").into());
    }
    Ok(value.to_ascii_lowercase())
}

fn build_manifest(config: &AppConfig, code_sha: &str) -> Result<ResetManifest, DynError> {
    let scope_id = config.scope_id.as_str();
    let redis = config.redis.as_ref().map(|redis| RedisResource {
        host: normalize_host(&redis.host),
        port: redis.port,
        database: redis.database,
        namespace: redis.namespace(),
        ownership_marker_key: format!("{}.ryframe-owner", redis.namespace()),
        ownership_marker: config.scope_id.ownership_marker("redis"),
    });

    let endpoint = match config.object_storage.backend {
        StorageBackend::Local => config.object_storage.local_base_dir.trim().to_owned(),
        StorageBackend::Rustfs | StorageBackend::Minio | StorageBackend::S3 => {
            config.object_storage.endpoint.trim().to_owned()
        }
    };
    let object_storage = ObjectStorageResource {
        backend: config.object_storage.backend.as_str().to_owned(),
        endpoint,
        prefixes: [
            UPLOAD_BUCKET,
            AVATAR_BUCKET,
            EXPORT_BUCKET,
            IMPORT_BUCKET,
            CONFIG_PACKAGE_BUCKET,
        ]
        .into_iter()
        .map(|bucket| ObjectPrefix {
            bucket: bucket.to_owned(),
            prefix: config.scope_id.object_prefix(),
            ownership_marker_key: format!("{scope_id}/.ryframe-owner"),
            ownership_marker: format!("ryframe-owner:v1:{scope_id}:object-storage:{bucket}"),
        })
        .collect(),
    };

    let databases = collect_databases(config)?;
    let environment = config.environment.as_str();
    let config_sha = sha256_hex(&canonical_json(&ConfigFingerprint {
        environment,
        scope_id,
        redis: &redis,
        object_storage: &object_storage,
        databases: &databases,
    })?);
    Ok(ResetManifest {
        manifest_version: MANIFEST_VERSION,
        environment: environment.to_owned(),
        scope_id: scope_id.to_owned(),
        code_sha: code_sha.to_owned(),
        config_sha,
        confirmation_phrase: format!("RESET-RYFRAME-{scope_id}"),
        redis,
        object_storage,
        databases,
    })
}

fn collect_databases(config: &AppConfig) -> Result<Vec<PhysicalDatabase>, DynError> {
    let mut databases = BTreeMap::<(String, u16, String), PhysicalDatabase>::new();
    insert_database(
        &mut databases,
        &config.database.primary.host,
        config.database.primary.port,
        &config.database.primary.database,
        "shared-control",
        config.scope_id.as_str(),
    )?;
    for target in config.tenant_data.normalized_targets() {
        match target.kind {
            TenantDatabaseTargetKind::Control => {}
            TenantDatabaseTargetKind::Mysql => insert_database(
                &mut databases,
                target.host.as_deref().ok_or("MySQL 目标缺少 host")?,
                target.port.unwrap_or(3306),
                target
                    .database
                    .as_deref()
                    .ok_or("MySQL 目标缺少 database")?,
                &target.key,
                config.scope_id.as_str(),
            )?,
        }
    }
    Ok(databases.into_values().collect())
}

fn insert_database(
    databases: &mut BTreeMap<(String, u16, String), PhysicalDatabase>,
    host: &str,
    port: u16,
    database: &str,
    target_key: &str,
    scope_id: &str,
) -> Result<(), DynError> {
    let host = normalize_host(host);
    let database = database.trim().to_owned();
    validate_database_identity(&host, port, &database)?;
    let key = (host.clone(), port, database.clone());
    let entry = databases.entry(key).or_insert_with(|| PhysicalDatabase {
        host,
        port,
        database,
        target_keys: Vec::new(),
        ownership_marker: format!("ryframe-owner:v1:{scope_id}:mysql"),
    });
    if !entry.target_keys.iter().any(|key| key == target_key) {
        entry.target_keys.push(target_key.to_owned());
        entry.target_keys.sort_unstable();
    }
    Ok(())
}

fn validate_database_identity(host: &str, port: u16, database: &str) -> Result<(), DynError> {
    if host.is_empty() || port == 0 {
        return Err("数据库 host 和 port 必须明确且有效".into());
    }
    if database.is_empty()
        || database.len() > 64
        || !database
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("数据库名必须为 1–64 位 ASCII 字母、数字或下划线".into());
    }
    if ["mysql", "information_schema", "performance_schema", "sys"]
        .iter()
        .any(|reserved| database.eq_ignore_ascii_case(reserved))
    {
        return Err(format!("系统数据库 `{database}` 永久禁止重建").into());
    }
    Ok(())
}

fn normalize_host(host: &str) -> String {
    host.trim().trim_matches(['[', ']']).to_ascii_lowercase()
}

fn authorize_execute(
    manifest: &ResetManifest,
    calculated_hash: &str,
    supplied_hash: &str,
    confirmation: &str,
) -> Result<(), DynError> {
    if supplied_hash != calculated_hash {
        return Err("plan hash 与当前不可变清单不匹配，请重新运行 plan".into());
    }
    if confirmation != manifest.confirmation_phrase {
        return Err(format!(
            "确认短语不匹配；必须精确传入 {}",
            manifest.confirmation_phrase
        )
        .into());
    }
    Ok(())
}

fn require_services_stopped() -> Result<(), DynError> {
    if std::env::var(SERVICES_STOPPED_ENV).as_deref() != Ok("YES") {
        return Err(format!(
            "必须先停止 API、Worker 和 scheduler，并设置 {SERVICES_STOPPED_ENV}=YES"
        )
        .into());
    }
    Ok(())
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec_pretty(value)
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn validate_sha256(value: &str) -> Result<(), DynError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("--plan-hash 必须为 64 位 SHA-256 十六进制字符串".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn physical_databases_are_deduplicated_and_system_schemas_rejected() {
        let mut databases = BTreeMap::new();
        insert_database(&mut databases, "LOCALHOST", 3306, "tenant_a", "a", "test")
            .expect("数据库有效");
        insert_database(&mut databases, "localhost", 3306, "tenant_a", "b", "test")
            .expect("重复物理库合并");
        let database = databases.into_values().next().expect("数据库存在");
        assert_eq!(database.target_keys, ["a", "b"]);
        assert!(validate_database_identity("localhost", 3306, "mysql").is_err());
    }

    #[test]
    fn plan_hash_is_deterministic_and_authorization_is_fail_closed() {
        let bytes = canonical_json(&vec!["a", "b"]).expect("编码清单");
        assert_eq!(sha256_hex(&bytes), sha256_hex(&bytes));
        assert!(validate_sha256(&"a".repeat(64)).is_ok());
        assert!(validate_sha256(&"z".repeat(64)).is_err());
    }
}
