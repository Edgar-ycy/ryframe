use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use ryframe_application::system::{
    AVATAR_BUCKET, CONFIG_PACKAGE_BUCKET, EXPORT_BUCKET, IMPORT_BUCKET, UPLOAD_BUCKET,
};
use ryframe_config::{
    AppConfig, DbConnection, DbTlsMode, StorageBackend, TenantDatabaseTargetKind,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{ResetError, ResetResult};

pub const MANIFEST_VERSION: u32 = 4;
pub const PRIMARY_DATABASE_PASSWORD_ENV: &str = "APP_DATABASE_PASSWORD";
pub const REDIS_PASSWORD_ENV: &str = "APP_REDIS_PASSWORD";
pub const OBJECT_STORAGE_ACCESS_KEY_ENV: &str = "APP_OBJECT_STORAGE_ACCESS_KEY";
pub const OBJECT_STORAGE_SECRET_KEY_ENV: &str = "APP_OBJECT_STORAGE_SECRET_KEY";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResetManifest {
    pub manifest_version: u32,
    pub environment: String,
    pub scope_id: String,
    pub code_sha: String,
    pub config_sha: String,
    pub credential_version: String,
    pub confirmation_phrase: String,
    pub legacy_ownership: LegacyOwnershipPolicy,
    pub redis: Option<RedisResource>,
    pub object_storage: ObjectStorageResource,
    pub databases: Vec<PhysicalDatabase>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LegacyOwnershipPolicy {
    pub mysql_exclusive: bool,
    pub redis_exclusive: bool,
    pub object_storage_exclusive: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RedisResource {
    pub host: String,
    pub port: u16,
    pub database: u8,
    pub namespace: String,
    pub ownership_marker_key: String,
    pub ownership_marker: String,
    pub outside_sentinel_key_sha256: String,
    pub password_env: Option<String>,
    pub tls: bool,
    pub tls_ca_sha256: Option<String>,
    pub tls_client_cert_sha256: Option<String>,
    pub tls_client_key_ref_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ObjectStorageResource {
    pub backend: String,
    pub endpoint: String,
    pub use_ssl: bool,
    pub region: String,
    pub access_key_env: Option<String>,
    pub secret_key_env: Option<String>,
    pub prefixes: Vec<ObjectPrefix>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ObjectPrefix {
    pub bucket: String,
    pub prefix: String,
    pub ownership_marker_key: String,
    pub ownership_marker: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PhysicalDatabase {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub connection: DatabaseConnectionIdentity,
    pub target_keys: Vec<String>,
    pub control_baseline: bool,
    pub tenant_baseline: bool,
    pub ownership_markers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DatabaseConnectionIdentity {
    pub username: String,
    pub password_env: String,
    pub tls_mode: String,
    pub tls_ca_sha256: Option<String>,
    pub tls_client_cert_sha256: Option<String>,
    pub tls_client_key_ref_sha256: Option<String>,
}

#[derive(Serialize)]
struct ConfigFingerprint<'a> {
    environment: &'a str,
    scope_id: &'a str,
    credential_version: &'a str,
    legacy_ownership: &'a LegacyOwnershipPolicy,
    redis: &'a Option<RedisResource>,
    object_storage: &'a ObjectStorageResource,
    databases: &'a [PhysicalDatabase],
}

pub fn build_manifest(config: &AppConfig, code_sha: &str) -> ResetResult<ResetManifest> {
    let scope_id = config.scope_id.as_str();
    let credential_version = config.reset.credential_version.as_deref().ok_or_else(|| {
        ResetError::new("reset plan 必须配置非秘密 reset.credential_version；凭据轮换时必须变更")
    })?;
    let redis = match config.redis.as_ref() {
        Some(redis) => {
            let sentinel = config
                .reset
                .redis_outside_sentinel_key
                .as_deref()
                .ok_or_else(|| {
                    ResetError::new(
                        "启用 Redis 时必须配置 reset.redis_outside_sentinel_key，以便验证 scope 外资源未变化",
                    )
                })?;
            if sentinel.starts_with(&redis.namespace()) {
                return Err(ResetError::new(
                    "Redis scope 外哨兵键不能位于当前 namespace 内",
                ));
            }
            Some(RedisResource {
                host: normalize_host(&redis.host),
                port: redis.port,
                database: redis.database,
                namespace: redis.namespace(),
                ownership_marker_key: format!("{}.ryframe-owner", redis.namespace()),
                ownership_marker: config.scope_id.ownership_marker("redis"),
                outside_sentinel_key_sha256: sha256_hex(sentinel.as_bytes()),
                password_env: (!redis.password.is_empty()).then(|| REDIS_PASSWORD_ENV.into()),
                tls: redis.tls,
                tls_ca_sha256: public_material_sha256(redis.tls_ca.as_deref())?,
                tls_client_cert_sha256: public_material_sha256(redis.tls_client_cert.as_deref())?,
                tls_client_key_ref_sha256: secret_reference_sha256(
                    redis.tls_client_key.as_deref(),
                )?,
            })
        }
        None => None,
    };

    let endpoint = match config.object_storage.backend {
        StorageBackend::Local => {
            canonical_local_storage_root(&config.object_storage.local_base_dir)?
        }
        StorageBackend::Rustfs | StorageBackend::Minio | StorageBackend::S3 => {
            config.object_storage.endpoint.trim().to_owned()
        }
    };
    let object_storage = ObjectStorageResource {
        backend: config.object_storage.backend.as_str().to_owned(),
        endpoint,
        use_ssl: config.object_storage.use_ssl,
        region: config.object_storage.region.trim().to_owned(),
        access_key_env: (config.object_storage.backend != StorageBackend::Local)
            .then(|| OBJECT_STORAGE_ACCESS_KEY_ENV.into()),
        secret_key_env: (config.object_storage.backend != StorageBackend::Local)
            .then(|| OBJECT_STORAGE_SECRET_KEY_ENV.into()),
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

    let legacy_ownership = LegacyOwnershipPolicy {
        mysql_exclusive: config.reset.legacy_mysql_exclusive,
        redis_exclusive: config.reset.legacy_redis_exclusive,
        object_storage_exclusive: config.reset.legacy_object_storage_exclusive,
    };
    let databases = collect_databases(config)?;
    let environment = config.environment.as_str();
    let config_sha = sha256_hex(&canonical_json(&ConfigFingerprint {
        environment,
        scope_id,
        credential_version,
        legacy_ownership: &legacy_ownership,
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
        credential_version: credential_version.to_owned(),
        confirmation_phrase: format!("RESET-RYFRAME-{environment}-{scope_id}"),
        legacy_ownership,
        redis,
        object_storage,
        databases,
    })
}

fn canonical_local_storage_root(configured: &str) -> ResetResult<String> {
    let path = Path::new(configured.trim());
    if !path.is_absolute() {
        return Err(ResetError::new(
            "destructive reset 要求 object_storage.local_base_dir 使用绝对路径",
        ));
    }
    let link_metadata = std::fs::symlink_metadata(path)
        .map_err(|_| ResetError::new("无法读取本地对象存储根目录身份"))?;
    if is_link_or_reparse(&link_metadata) {
        return Err(ResetError::new("本地对象存储根目录不能是符号链接"));
    }
    let canonical =
        std::fs::canonicalize(path).map_err(|_| ResetError::new("无法规范化本地对象存储根目录"))?;
    if !canonical.is_dir() {
        return Err(ResetError::new("本地对象存储根路径必须是已存在目录"));
    }
    canonical
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| ResetError::new("本地对象存储规范路径必须使用 Unicode 编码"))
}

fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn collect_databases(config: &AppConfig) -> ResetResult<Vec<PhysicalDatabase>> {
    let mut databases = BTreeMap::<(String, u16, String), PhysicalDatabase>::new();
    insert_database(
        &mut databases,
        &config.database.primary.host,
        config.database.primary.port,
        &config.database.primary.database,
        database_connection_identity(&config.database.primary, PRIMARY_DATABASE_PASSWORD_ENV)?,
        "shared-control",
        true,
        true,
        config.scope_id.as_str(),
    )?;
    for target in &config.tenant_data.targets {
        if target.kind == TenantDatabaseTargetKind::Mysql {
            let password_env = target
                .password_env
                .as_deref()
                .ok_or_else(|| ResetError::new("MySQL 目标缺少 password_env"))?;
            let connection = DbConnection {
                host: target
                    .host
                    .clone()
                    .ok_or_else(|| ResetError::new("MySQL 目标缺少 host"))?,
                port: target.port.unwrap_or(3306),
                database: target
                    .database
                    .clone()
                    .ok_or_else(|| ResetError::new("MySQL 目标缺少 database"))?,
                username: target
                    .username
                    .clone()
                    .ok_or_else(|| ResetError::new("MySQL 目标缺少 username"))?,
                password: String::new(),
                max_connections: target.max_connections.unwrap_or(2).min(4),
                min_connections: 0,
                acquire_timeout_secs: 10,
                idle_timeout_secs: 60,
                max_lifetime_secs: 300,
                connect_timeout_secs: 10,
                tls_mode: target.tls_mode.unwrap_or_default(),
                tls_ca: target.tls_ca.clone(),
                tls_client_cert: target.tls_client_cert.clone(),
                tls_client_key: target.tls_client_key.clone(),
            };
            insert_database(
                &mut databases,
                &connection.host,
                connection.port,
                &connection.database,
                database_connection_identity(&connection, password_env)?,
                &target.key,
                false,
                true,
                config.scope_id.as_str(),
            )?;
        }
    }
    Ok(databases.into_values().collect())
}

#[allow(clippy::too_many_arguments)]
fn insert_database(
    databases: &mut BTreeMap<(String, u16, String), PhysicalDatabase>,
    host: &str,
    port: u16,
    database: &str,
    connection: DatabaseConnectionIdentity,
    target_key: &str,
    control_baseline: bool,
    tenant_baseline: bool,
    scope_id: &str,
) -> ResetResult<()> {
    let host = normalize_host(host);
    let database = database.trim().to_owned();
    validate_database_identity(&host, port, &database)?;
    let key = (host.clone(), port, database.clone());
    if let Some(existing) = databases.get(&key)
        && existing.connection != connection
    {
        return Err(ResetError::new(
            "同一物理数据库配置了不同的非秘密连接身份，拒绝生成歧义清单",
        ));
    }
    let entry = databases.entry(key).or_insert_with(|| PhysicalDatabase {
        host,
        port,
        database,
        connection,
        target_keys: Vec::new(),
        control_baseline: false,
        tenant_baseline: false,
        ownership_markers: BTreeMap::new(),
    });
    entry.control_baseline |= control_baseline;
    entry.tenant_baseline |= tenant_baseline;
    if !entry.target_keys.iter().any(|key| key == target_key) {
        entry.target_keys.push(target_key.to_owned());
        entry.target_keys.sort_unstable();
    }
    if control_baseline {
        entry.ownership_markers.insert(
            "control".into(),
            ryframe_db::resource_ownership::marker(scope_id, "control"),
        );
    }
    if tenant_baseline {
        entry.ownership_markers.insert(
            "tenant-data".into(),
            ryframe_db::resource_ownership::marker(scope_id, "tenant-data"),
        );
    }
    Ok(())
}

pub fn database_connection_identity(
    connection: &DbConnection,
    password_env: &str,
) -> ResetResult<DatabaseConnectionIdentity> {
    if connection.username.trim().is_empty()
        || connection.username != connection.username.trim()
        || password_env.trim().is_empty()
        || password_env != password_env.trim()
    {
        return Err(ResetError::new(
            "数据库 username 与 password_env 必须明确且不含边界空白",
        ));
    }
    Ok(DatabaseConnectionIdentity {
        username: connection.username.clone(),
        password_env: password_env.to_owned(),
        tls_mode: tls_mode_name(connection.tls_mode).into(),
        tls_ca_sha256: public_material_sha256(connection.tls_ca.as_deref())?,
        tls_client_cert_sha256: public_material_sha256(connection.tls_client_cert.as_deref())?,
        tls_client_key_ref_sha256: secret_reference_sha256(connection.tls_client_key.as_deref())?,
    })
}

fn tls_mode_name(mode: DbTlsMode) -> &'static str {
    match mode {
        DbTlsMode::Disabled => "disabled",
        DbTlsMode::Required => "required",
        DbTlsMode::VerifyCa => "verify_ca",
        DbTlsMode::VerifyIdentity => "verify_identity",
    }
}

pub(crate) fn public_material_sha256(reference: Option<&str>) -> ResetResult<Option<String>> {
    let Some(path) = canonical_file_reference(reference)? else {
        return Ok(None);
    };
    let bytes = std::fs::read(&path).map_err(|_| ResetError::new("无法读取 TLS 公共证书材料"))?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(ResetError::new("TLS 公共证书材料超过 4 MiB 安全上限"));
    }
    Ok(Some(sha256_hex(&bytes)))
}

pub(crate) fn secret_reference_sha256(reference: Option<&str>) -> ResetResult<Option<String>> {
    match canonical_file_reference(reference)? {
        Some(path) => path
            .to_str()
            .map(|value| Some(sha256_hex(value.as_bytes())))
            .ok_or_else(|| ResetError::new("reset TLS 私钥引用必须使用 Unicode 路径")),
        None => Ok(None),
    }
}

fn canonical_file_reference(reference: Option<&str>) -> ResetResult<Option<std::path::PathBuf>> {
    let Some(reference) = reference.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let path = Path::new(reference);
    if !path.is_absolute() {
        return Err(ResetError::new("reset 使用的 TLS 文件引用必须是绝对路径"));
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| ResetError::new("无法读取 reset TLS 文件身份"))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(ResetError::new(
            "reset TLS 文件不能是链接、重解析点或非文件",
        ));
    }
    std::fs::canonicalize(path)
        .map(Some)
        .map_err(|_| ResetError::new("无法规范化 reset TLS 文件引用"))
}

pub fn validate_database_identity(host: &str, port: u16, database: &str) -> ResetResult<()> {
    if host.is_empty() || port == 0 {
        return Err(ResetError::new("数据库 host 和 port 必须明确且有效"));
    }
    if database.is_empty()
        || database.len() > 64
        || !database
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(ResetError::new(
            "数据库名必须为 1–64 位 ASCII 字母、数字或下划线",
        ));
    }
    if ["mysql", "information_schema", "performance_schema", "sys"]
        .iter()
        .any(|reserved| database.eq_ignore_ascii_case(reserved))
    {
        return Err(ResetError::new(format!(
            "系统数据库 `{database}` 永久禁止重建"
        )));
    }
    Ok(())
}

pub fn validate_database_set(manifest: &ResetManifest) -> ResetResult<()> {
    let mut identities = BTreeSet::new();
    let mut control_count = 0;
    for database in &manifest.databases {
        validate_database_identity(&database.host, database.port, &database.database)?;
        if !identities.insert((&database.host, database.port, &database.database)) {
            return Err(ResetError::new("不可变清单包含重复物理数据库"));
        }
        control_count += usize::from(database.control_baseline);
        if database.target_keys.is_empty() || !database.tenant_baseline {
            return Err(ResetError::new("数据库清单缺少租户基线角色"));
        }
    }
    if control_count != 1 {
        return Err(ResetError::new("数据库清单必须且只能包含一个控制库"));
    }
    Ok(())
}

pub fn object_resource_key(item: &ObjectPrefix) -> String {
    format!("object:{}:{}", item.bucket, item.prefix)
}

pub fn redis_resource_key(resource: &RedisResource) -> String {
    format!(
        "redis:{}:{}:{}:{}",
        resource.host, resource.port, resource.database, resource.namespace
    )
}

pub fn database_resource_key(resource: &PhysicalDatabase) -> String {
    format!(
        "mysql:{}:{}/{}",
        resource.host, resource.port, resource.database
    )
}

pub fn resource_keys(manifest: &ResetManifest) -> BTreeSet<String> {
    let mut keys = manifest
        .object_storage
        .prefixes
        .iter()
        .map(object_resource_key)
        .collect::<BTreeSet<_>>();
    if let Some(redis) = &manifest.redis {
        keys.insert(redis_resource_key(redis));
    }
    keys.extend(manifest.databases.iter().map(database_resource_key));
    keys
}

pub fn normalize_host(host: &str) -> String {
    host.trim().trim_matches(['[', ']']).to_ascii_lowercase()
}

pub fn canonical_json<T: Serialize>(value: &T) -> ResetResult<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(|_| ResetError::new("无法编码无秘密 reset 清单"))
}

pub fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection_identity(password_env: &str) -> DatabaseConnectionIdentity {
        DatabaseConnectionIdentity {
            username: "reset".into(),
            password_env: password_env.into(),
            tls_mode: "verify_identity".into(),
            tls_ca_sha256: Some("a".repeat(64)),
            tls_client_cert_sha256: None,
            tls_client_key_ref_sha256: None,
        }
    }

    #[test]
    fn physical_databases_are_deduplicated_and_roles_are_merged() {
        let mut databases = BTreeMap::new();
        insert_database(
            &mut databases,
            "LOCALHOST",
            3306,
            "tenant_a",
            connection_identity("TENANT_A_PASSWORD"),
            "shared-control",
            true,
            true,
            "test",
        )
        .expect("数据库有效");
        insert_database(
            &mut databases,
            "localhost",
            3306,
            "tenant_a",
            connection_identity("TENANT_A_PASSWORD"),
            "tenant-a",
            false,
            true,
            "test",
        )
        .expect("重复物理库合并");
        let database = databases.into_values().next().expect("数据库存在");
        assert_eq!(database.target_keys, ["shared-control", "tenant-a"]);
        assert!(database.control_baseline);
        assert!(database.tenant_baseline);
        assert_eq!(database.ownership_markers.len(), 2);
    }

    #[test]
    fn connection_identity_is_part_of_the_plan_hash_and_deduplication_key() {
        let baseline = connection_identity("TENANT_A_PASSWORD");
        let changed = connection_identity("TENANT_B_PASSWORD");
        assert_ne!(
            sha256_hex(&canonical_json(&baseline).expect("编码连接身份")),
            sha256_hex(&canonical_json(&changed).expect("编码连接身份"))
        );

        let mut databases = BTreeMap::new();
        insert_database(
            &mut databases,
            "localhost",
            3306,
            "tenant_a",
            baseline,
            "shared-control",
            true,
            true,
            "test",
        )
        .expect("首次登记数据库");
        assert!(
            insert_database(
                &mut databases,
                "localhost",
                3306,
                "tenant_a",
                changed,
                "tenant-a",
                false,
                true,
                "test",
            )
            .is_err()
        );
    }

    #[test]
    fn system_schemas_and_unsafe_identifiers_are_rejected() {
        assert!(validate_database_identity("localhost", 3306, "mysql").is_err());
        assert!(validate_database_identity("localhost", 3306, "tenant-a").is_err());
        assert!(validate_database_identity("", 3306, "tenant_a").is_err());
    }

    #[test]
    fn canonical_hash_is_deterministic() {
        let bytes = canonical_json(&vec!["a", "b"]).expect("编码清单");
        assert_eq!(sha256_hex(&bytes), sha256_hex(&bytes));
    }
}
