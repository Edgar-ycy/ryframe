use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use ryframe_config::{AppConfig, DbConnection, DbTlsMode, TenantDatabaseTargetKind};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait, TryGetable,
};

use crate::reset::{
    ResetError, ResetResult,
    engine::{PhaseEvidence, ResourceProgress},
    ledger::ResetLedger,
    model::{
        PRIMARY_DATABASE_PASSWORD_ENV, PhysicalDatabase, ResetManifest,
        database_connection_identity, database_resource_key, normalize_host, sha256_hex,
    },
};

const ADMIN_PASSWORD_ENV: &str = "RYFRAME_RESET_ADMIN_PASSWORD";
const USER_PASSWORD_ENV: &str = "RYFRAME_RESET_USER_PASSWORD";
const EMPTY_CONTROL_TABLES: &[&str] = &[
    "sys_background_job",
    "sys_export_job",
    "sys_file",
    "sys_outbox_event",
    "sys_oper_log",
    "sys_login_info",
    "sys_user_import_job",
    "sys_user_import_row_result",
];
const REQUIRED_GLOBAL_PRIVILEGES: &[&str] = &[
    "ALTER",
    "CREATE",
    "DELETE",
    "DROP",
    "INDEX",
    "INSERT",
    "REFERENCES",
    "SELECT",
    "TRIGGER",
    "UPDATE",
];

pub struct MysqlReset {
    databases: Vec<DatabaseHandle>,
    locks: Vec<ServerLock>,
    seeds: Option<SeedCredentials>,
}

struct DatabaseHandle {
    resource: PhysicalDatabase,
    target: DbConnection,
    server: DatabaseConnection,
    server_uuid: String,
    lower_case_table_names: i64,
}

struct ServerLock {
    transaction: DatabaseTransaction,
    key: String,
    server_uuid: String,
}

struct SeedCredentials {
    admin_password: String,
    admin_hash: String,
    user_password: String,
    user_hash: String,
}

#[derive(Clone)]
struct DatabaseSpec {
    resource: PhysicalDatabase,
    connection: DbConnection,
}

impl Default for MysqlReset {
    fn default() -> Self {
        Self::new()
    }
}

impl MysqlReset {
    pub const fn new() -> Self {
        Self {
            databases: Vec::new(),
            locks: Vec::new(),
            seeds: None,
        }
    }

    pub async fn preflight(
        &mut self,
        config: &AppConfig,
        manifest: &ResetManifest,
        ledger: &ResetLedger,
    ) -> ResetResult<PhaseEvidence> {
        if !self.databases.is_empty() || !self.locks.is_empty() {
            return Err(ResetError::new("MySQL reset runtime 被重复预检"));
        }
        self.seeds = Some(load_seed_credentials()?);
        let specs = collect_specs(config, manifest)?;
        for spec in specs {
            let mut server_config = spec.connection.clone();
            server_config.database = "information_schema".into();
            server_config.max_connections = 2;
            server_config.min_connections = 0;
            let server = ryframe_db::connection::connect(&server_config)
                .await
                .map_err(|_| {
                    ResetError::new(format!(
                        "无法连接 MySQL server {}:{}",
                        spec.resource.host, spec.resource.port
                    ))
                })?;
            let (server_uuid, lower_case_table_names) = server_identity(&server).await?;
            self.databases.push(DatabaseHandle {
                resource: spec.resource,
                target: spec.connection,
                server,
                server_uuid,
                lower_case_table_names,
            });
        }

        validate_physical_database_identities(&self.databases)?;
        self.acquire_server_locks(manifest).await?;
        for handle in &self.databases {
            ryframe_tenant_db::migration::verify_mysql_80(&handle.server)
                .await
                .map_err(|_| ResetError::new("MySQL 版本预检失败，要求 MySQL 8.0.16 或更高"))?;
            verify_server_writable(&handle.server).await?;
            verify_global_privileges(&handle.server).await?;
            let resource_key = database_resource_key(&handle.resource);
            let resource_started = ledger.resource_started(&resource_key);
            let physical_identity = database_physical_identity(handle);
            match ledger.resource_identity(&resource_key) {
                Some(recorded) if recorded != physical_identity.as_str() => {
                    return Err(ResetError::new(
                        "MySQL 物理数据库身份与耐久 reset 进度不一致，拒绝续跑",
                    ));
                }
                None if resource_started => {
                    return Err(ResetError::new(
                        "MySQL reset 进度缺少物理数据库身份，拒绝放宽所有权校验",
                    ));
                }
                _ => {}
            }
            let lock = self.lock_for_server(&handle.server_uuid)?;
            verify_database_ownership_before_recreate(
                handle,
                &lock.transaction,
                manifest.legacy_ownership.mysql_exclusive,
                resource_started,
            )
            .await?;
        }
        Ok(PhaseEvidence::from([
            ("database_count".into(), self.databases.len().to_string()),
            ("server_lock_count".into(), self.locks.len().to_string()),
            ("ddl_privileges".into(), "verified".into()),
            ("seed_passwords".into(), "loaded_and_hashed".into()),
        ]))
    }

    pub async fn recreate(
        &self,
        manifest: &ResetManifest,
        progress: &mut ResourceProgress<'_>,
    ) -> ResetResult<PhaseEvidence> {
        self.assert_locks_held().await?;
        let mut recreated = 0_usize;
        for handle in &self.databases {
            let resource_key = database_resource_key(&handle.resource);
            let physical_identity = database_physical_identity(handle);
            if progress.is_complete(&resource_key) {
                continue;
            }
            if let Some(recorded) = progress.identity(&resource_key)
                && recorded != physical_identity.as_str()
            {
                return Err(ResetError::new(
                    "MySQL 物理数据库身份在重建前发生变化，拒绝执行 DROP",
                ));
            }
            self.assert_locks_held().await?;
            let lock = self.lock_for_server(&handle.server_uuid)?;
            verify_database_ownership_before_recreate(
                handle,
                &lock.transaction,
                manifest.legacy_ownership.mysql_exclusive,
                progress.is_started(&resource_key),
            )
            .await?;
            progress.begin_with_identity(&resource_key, &physical_identity)?;
            let quoted = quote_identifier(&handle.resource.database)?;
            self.assert_locks_held().await?;
            lock.transaction
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted}"))
                .await
                .map_err(|_| ResetError::new("删除目标数据库失败"))?;
            self.assert_locks_held().await?;
            lock.transaction
                .execute_unprepared(&format!(
                    "CREATE DATABASE {quoted} CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci"
                ))
                .await
                .map_err(|_| ResetError::new("重新创建目标数据库失败"))?;
            if !database_exists(&lock.transaction, &handle.resource.database).await? {
                return Err(ResetError::new("目标数据库重建后不存在"));
            }
            progress.complete(&resource_key)?;
            recreated += 1;
        }
        Ok(PhaseEvidence::from([(
            "recreated_databases".into(),
            recreated.to_string(),
        )]))
    }

    pub async fn migrate_control(&self, manifest: &ResetManifest) -> ResetResult<PhaseEvidence> {
        self.assert_locks_held().await?;
        let handle = self
            .databases
            .iter()
            .find(|handle| handle.resource.control_baseline)
            .ok_or_else(|| ResetError::new("不可变清单缺少控制库"))?;
        let target = self.connect_verified_target(handle).await?;
        ryframe_db::migration::up(&target)
            .await
            .map_err(|_| ResetError::new("控制库唯一 baseline 执行失败"))?;
        let seeds = self
            .seeds
            .as_ref()
            .ok_or_else(|| ResetError::new("种子密码未完成预检"))?;
        write_seed_hashes(&target, seeds).await?;
        ryframe_db::resource_ownership::ensure_resource_ownership(
            &target,
            &manifest.scope_id,
            "control",
        )
        .await
        .map_err(|_| ResetError::new("控制库所有权 marker 写入失败"))?;
        target
            .close()
            .await
            .map_err(|_| ResetError::new("关闭控制库迁移连接失败"))?;
        Ok(PhaseEvidence::from([
            ("control_baseline".into(), "1".into()),
            (
                "schema_fingerprint".into(),
                ryframe_db::migration::schema_fingerprint(),
            ),
        ]))
    }

    pub async fn migrate_tenants(&self, manifest: &ResetManifest) -> ResetResult<PhaseEvidence> {
        self.assert_locks_held().await?;
        let mut migrated = 0_usize;
        for handle in &self.databases {
            if !handle.resource.tenant_baseline {
                continue;
            }
            let target = self.connect_verified_target(handle).await?;
            ryframe_tenant_db::migration::up(&target)
                .await
                .map_err(|_| ResetError::new("租户库唯一 baseline 执行失败"))?;
            ryframe_db::resource_ownership::ensure_resource_ownership(
                &target,
                &manifest.scope_id,
                "tenant-data",
            )
            .await
            .map_err(|_| ResetError::new("租户库所有权 marker 写入失败"))?;
            target
                .close()
                .await
                .map_err(|_| ResetError::new("关闭租户库迁移连接失败"))?;
            migrated += 1;
        }
        Ok(PhaseEvidence::from([
            ("tenant_baselines".into(), migrated.to_string()),
            (
                "schema_fingerprint".into(),
                ryframe_tenant_db::migration::TENANT_DATA_SCHEMA_FINGERPRINT.into(),
            ),
        ]))
    }

    pub async fn verify(&self, manifest: &ResetManifest) -> ResetResult<PhaseEvidence> {
        self.assert_locks_held().await?;
        let seeds = self
            .seeds
            .as_ref()
            .ok_or_else(|| ResetError::new("种子密码未完成预检"))?;
        let mut verified_tenants = 0_usize;
        for handle in &self.databases {
            let target = self.connect_verified_target(handle).await?;
            if handle.resource.control_baseline {
                ryframe_db::migration::verify(&target)
                    .await
                    .map_err(|_| ResetError::new("控制库 ledger/schema 指纹验证失败"))?;
                ryframe_db::resource_ownership::verify_resource_ownership(
                    &target,
                    &manifest.scope_id,
                    "control",
                )
                .await
                .map_err(|_| ResetError::new("控制库所有权 marker 验证失败"))?;
                verify_seed_hashes(&target, seeds).await?;
                verify_empty_control_resources(&target).await?;
            }
            if handle.resource.tenant_baseline {
                ryframe_tenant_db::migration::verify(&target)
                    .await
                    .map_err(|_| ResetError::new("租户库 ledger/schema 指纹验证失败"))?;
                if !handle.resource.control_baseline {
                    ryframe_tenant_db::migration::verify_mysql_target(&target)
                        .await
                        .map_err(|_| ResetError::new("外部租户库边界验证失败"))?;
                    verify_empty_external_tenant(&target).await?;
                }
                ryframe_db::resource_ownership::verify_resource_ownership(
                    &target,
                    &manifest.scope_id,
                    "tenant-data",
                )
                .await
                .map_err(|_| ResetError::new("租户库所有权 marker 验证失败"))?;
                verified_tenants += 1;
            }
            target
                .close()
                .await
                .map_err(|_| ResetError::new("关闭 MySQL 验证连接失败"))?;
        }
        Ok(PhaseEvidence::from([
            ("verified_control".into(), "1".into()),
            (
                "verified_tenant_targets".into(),
                verified_tenants.to_string(),
            ),
            ("empty_runtime_resources".into(), "verified".into()),
        ]))
    }

    pub async fn release(&mut self) -> ResetResult<()> {
        let mut first_error = None;
        while let Some(lock) = self.locks.pop() {
            let ServerLock {
                transaction, key, ..
            } = lock;
            let release = scalar_i64(
                &transaction,
                "SELECT RELEASE_LOCK(?)",
                [key.as_str().into()],
            )
            .await;
            if !matches!(release, Ok(Some(1))) && first_error.is_none() {
                first_error = Some(ResetError::new("MySQL 环境级 reset lock 释放失败"));
            }
            if transaction.rollback().await.is_err() && first_error.is_none() {
                first_error = Some(ResetError::new("MySQL reset lock 连接关闭失败"));
            }
        }
        self.databases.clear();
        self.seeds = None;
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    async fn acquire_server_locks(&mut self, manifest: &ResetManifest) -> ResetResult<()> {
        let mut servers = BTreeSet::new();
        for handle in &self.databases {
            if !servers.insert(handle.server_uuid.as_str()) {
                continue;
            }
            let lock_hash =
                sha256_hex(format!("{}:{}", manifest.scope_id, handle.server_uuid).as_bytes());
            let key = format!("ryframe:reset:{}", &lock_hash[..48]);
            let transaction = handle
                .server
                .begin()
                .await
                .map_err(|_| ResetError::new("无法建立 MySQL reset lock 会话"))?;
            let acquired =
                scalar_i64(&transaction, "SELECT GET_LOCK(?, 0)", [key.as_str().into()]).await?;
            if acquired != Some(1) {
                let _ = transaction.rollback().await;
                return Err(ResetError::new(
                    "MySQL 环境级 reset lock 已被其他执行器持有",
                ));
            }
            self.locks.push(ServerLock {
                transaction,
                key,
                server_uuid: handle.server_uuid.clone(),
            });
        }
        Ok(())
    }

    pub async fn assert_locks_held(&self) -> ResetResult<()> {
        if self.locks.is_empty() {
            return Err(ResetError::new("MySQL reset lock 尚未持有"));
        }
        for lock in &self.locks {
            let held = scalar_i64(
                &lock.transaction,
                "SELECT IS_USED_LOCK(?) = CONNECTION_ID()",
                [lock.key.as_str().into()],
            )
            .await?;
            if held != Some(1) {
                return Err(ResetError::new("MySQL 环境级 reset lock 已丢失"));
            }
        }
        Ok(())
    }

    fn lock_for_server(&self, server_uuid: &str) -> ResetResult<&ServerLock> {
        self.locks
            .iter()
            .find(|lock| lock.server_uuid == server_uuid)
            .ok_or_else(|| ResetError::new("目标 MySQL server 缺少已持有的 reset lock"))
    }

    async fn connect_verified_target(
        &self,
        handle: &DatabaseHandle,
    ) -> ResetResult<DatabaseConnection> {
        self.assert_locks_held().await?;
        let target = connect_target(&handle.target, &handle.resource).await?;
        match server_identity(&target).await {
            Ok((server_uuid, lower_case_table_names))
                if server_uuid == handle.server_uuid
                    && lower_case_table_names == handle.lower_case_table_names => {}
            Ok(_) => {
                let _ = target.close().await;
                return Err(ResetError::new(
                    "MySQL 目标连接的物理 server 身份与预检不一致",
                ));
            }
            Err(error) => {
                let _ = target.close().await;
                return Err(error);
            }
        }
        self.assert_locks_held().await?;
        Ok(target)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnershipState {
    Verified,
    Missing,
    Mismatch,
}

async fn server_identity(db: &DatabaseConnection) -> ResetResult<(String, i64)> {
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT @@server_uuid, CAST(@@lower_case_table_names AS CHAR)",
        ))
        .await
        .map_err(|_| ResetError::new("无法读取 MySQL 物理 server 身份"))?
        .ok_or_else(|| ResetError::new("MySQL 物理 server 身份查询无结果"))?;
    let server_uuid = String::try_get_by_index(&row, 0)
        .map_err(|_| ResetError::new("MySQL server_uuid 格式无效"))?;
    let lower_case_table_names = String::try_get_by_index(&row, 1)
        .map_err(|_| ResetError::new("MySQL lower_case_table_names 格式无效"))?;
    let lower_case_table_names = parse_lower_case_table_names(&lower_case_table_names)?;
    if server_uuid.trim().is_empty()
        || server_uuid.len() > 64
        || !matches!(lower_case_table_names, 0..=2)
    {
        return Err(ResetError::new("MySQL 物理 server 身份值无效"));
    }
    Ok((server_uuid, lower_case_table_names))
}

pub fn parse_lower_case_table_names(value: &str) -> ResetResult<i64> {
    let parsed = value
        .parse::<i64>()
        .map_err(|_| ResetError::new("MySQL lower_case_table_names 格式无效"))?;
    if !matches!(parsed, 0..=2) {
        return Err(ResetError::new("MySQL lower_case_table_names 值无效"));
    }
    Ok(parsed)
}

fn validate_physical_database_identities(handles: &[DatabaseHandle]) -> ResetResult<()> {
    let mut identities = BTreeSet::new();
    let mut server_modes = BTreeMap::new();
    for handle in handles {
        if let Some(existing) =
            server_modes.insert(handle.server_uuid.as_str(), handle.lower_case_table_names)
            && existing != handle.lower_case_table_names
        {
            return Err(ResetError::new(
                "同一 MySQL server 返回了不一致的大小写模式",
            ));
        }
        let database = if handle.lower_case_table_names == 0 {
            handle.resource.database.clone()
        } else {
            handle.resource.database.to_ascii_lowercase()
        };
        if !identities.insert((handle.server_uuid.as_str(), database)) {
            return Err(ResetError::new(
                "多个 host/DNS 配置指向同一 MySQL 物理数据库，拒绝重复重建",
            ));
        }
    }
    Ok(())
}

fn database_physical_identity(handle: &DatabaseHandle) -> String {
    let database = if handle.lower_case_table_names == 0 {
        Cow::Borrowed(handle.resource.database.as_str())
    } else {
        Cow::Owned(handle.resource.database.to_ascii_lowercase())
    };
    sha256_hex(format!("mysql:{}:{database}", handle.server_uuid).as_bytes())
}

fn collect_specs(config: &AppConfig, manifest: &ResetManifest) -> ResetResult<Vec<DatabaseSpec>> {
    let mut specs = BTreeMap::<(String, u16, String), DatabaseSpec>::new();
    let primary_password = std::env::var(PRIMARY_DATABASE_PASSWORD_ENV)
        .map_err(|_| ResetError::new("控制库密码环境变量缺失或编码无效"))?;
    if primary_password != config.database.primary.password {
        return Err(ResetError::new(
            "控制库密码必须来自当前 APP_DATABASE_PASSWORD 环境变量",
        ));
    }
    let mut primary = config.database.primary.clone();
    primary.password = primary_password;
    insert_spec(&mut specs, primary, PRIMARY_DATABASE_PASSWORD_ENV, manifest)?;
    for target in &config.tenant_data.targets {
        if target.kind != TenantDatabaseTargetKind::Mysql {
            continue;
        }
        let password_env = target
            .password_env
            .as_deref()
            .ok_or_else(|| ResetError::new("MySQL 目标缺少 password_env"))?;
        let password = std::env::var(password_env)
            .map_err(|_| ResetError::new("MySQL 目标密码环境变量缺失或编码无效"))?;
        if password.is_empty() {
            return Err(ResetError::new("MySQL 目标密码环境变量不能为空"));
        }
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
            password,
            max_connections: target.max_connections.unwrap_or(2).min(4),
            min_connections: 0,
            acquire_timeout_secs: 10,
            idle_timeout_secs: 60,
            max_lifetime_secs: 300,
            connect_timeout_secs: 10,
            tls_mode: target.tls_mode.unwrap_or(DbTlsMode::Required),
            tls_ca: target.tls_ca.clone(),
            tls_client_cert: target.tls_client_cert.clone(),
            tls_client_key: target.tls_client_key.clone(),
        };
        insert_spec(&mut specs, connection, password_env, manifest)?;
    }
    if specs.len() != manifest.databases.len() {
        return Err(ResetError::new(
            "MySQL 运行时连接集合与不可变数据库清单不一致",
        ));
    }
    Ok(specs.into_values().collect())
}

fn insert_spec(
    specs: &mut BTreeMap<(String, u16, String), DatabaseSpec>,
    connection: DbConnection,
    password_env: &str,
    manifest: &ResetManifest,
) -> ResetResult<()> {
    let identity = (
        normalize_host(&connection.host),
        connection.port,
        connection.database.trim().to_owned(),
    );
    let resource = manifest
        .databases
        .iter()
        .find(|database| {
            database.host == identity.0
                && database.port == identity.1
                && database.database == identity.2
        })
        .ok_or_else(|| ResetError::new("MySQL 连接不属于不可变数据库清单"))?;
    if database_connection_identity(&connection, password_env)? != resource.connection {
        return Err(ResetError::new(
            "MySQL 非秘密连接参数与不可变清单不一致，请重新运行 plan",
        ));
    }
    if let Some(existing) = specs.get(&identity) {
        if !same_credentials(&existing.connection, &connection) {
            return Err(ResetError::new(
                "同一物理数据库配置了冲突的 MySQL 凭据或 TLS 参数",
            ));
        }
        return Ok(());
    }
    specs.insert(
        identity,
        DatabaseSpec {
            resource: resource.clone(),
            connection,
        },
    );
    Ok(())
}

pub fn same_credentials(left: &DbConnection, right: &DbConnection) -> bool {
    left.username == right.username
        && left.password == right.password
        && left.tls_mode == right.tls_mode
        && left.tls_ca == right.tls_ca
        && left.tls_client_cert == right.tls_client_cert
        && left.tls_client_key == right.tls_client_key
}

fn load_seed_credentials() -> ResetResult<SeedCredentials> {
    fn load(name: &str) -> ResetResult<(String, String)> {
        let password = std::env::var(name)
            .map_err(|_| ResetError::new("reset 种子密码环境变量缺失或编码无效"))?;
        ryframe_auth::password::validate_complexity(&password)
            .map_err(|_| ResetError::new("reset 种子密码不满足复杂度策略"))?;
        let hash = ryframe_auth::password::hash(&password)
            .map_err(|_| ResetError::new("reset 种子密码哈希失败"))?;
        Ok((password, hash))
    }
    let (admin_password, admin_hash) = load(ADMIN_PASSWORD_ENV)?;
    let (user_password, user_hash) = load(USER_PASSWORD_ENV)?;
    Ok(SeedCredentials {
        admin_password,
        admin_hash,
        user_password,
        user_hash,
    })
}

async fn connect_target(
    config: &DbConnection,
    resource: &PhysicalDatabase,
) -> ResetResult<DatabaseConnection> {
    ryframe_db::connection::connect(config).await.map_err(|_| {
        ResetError::new(format!(
            "无法连接目标数据库 {}:{}/{}",
            resource.host, resource.port, resource.database
        ))
    })
}

async fn database_exists<C>(db: &C, name: &str) -> ResetResult<bool>
where
    C: ConnectionTrait + ?Sized,
{
    Ok(scalar_i64(
        db,
        "SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name = ?",
        [name.into()],
    )
    .await?
        == Some(1))
}

async fn verify_database_ownership_before_recreate(
    handle: &DatabaseHandle,
    server: &DatabaseTransaction,
    legacy_exclusive: bool,
    resource_started: bool,
) -> ResetResult<()> {
    if !database_exists(server, &handle.resource.database).await? {
        return if legacy_exclusive || resource_started {
            Ok(())
        } else {
            Err(ResetError::new(format!(
                "数据库 {} 不存在且无法验证所有权；仅允许显式独占接管或同 manifest 续跑",
                handle.resource.database
            )))
        };
    }
    let ownership = inspect_database_ownership_on_server(server, &handle.resource).await?;
    match ownership {
        OwnershipState::Verified => Ok(()),
        OwnershipState::Missing if legacy_exclusive || resource_started => Ok(()),
        OwnershipState::Missing => Err(ResetError::new(format!(
            "数据库 {} 缺少 scope marker；仅能通过明确的 dev/test 旧资源独占配置接管",
            handle.resource.database
        ))),
        OwnershipState::Mismatch => Err(ResetError::new(format!(
            "数据库 {} 的 scope marker 不匹配",
            handle.resource.database
        ))),
    }
}

async fn verify_server_writable(db: &DatabaseConnection) -> ResetResult<()> {
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT @@global.read_only, @@global.super_read_only",
        ))
        .await
        .map_err(|_| ResetError::new("无法读取 MySQL 只读状态"))?
        .ok_or_else(|| ResetError::new("MySQL 只读状态查询无结果"))?;
    let read_only = i64::try_get_by_index(&row, 0)
        .map_err(|_| ResetError::new("MySQL read_only 状态格式无效"))?;
    let super_read_only = i64::try_get_by_index(&row, 1)
        .map_err(|_| ResetError::new("MySQL super_read_only 状态格式无效"))?;
    if read_only != 0 || super_read_only != 0 {
        return Err(ResetError::new("MySQL server 为只读目标，永久拒绝重建"));
    }
    Ok(())
}

async fn verify_global_privileges(db: &DatabaseConnection) -> ResetResult<()> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SHOW GRANTS FOR CURRENT_USER()",
        ))
        .await
        .map_err(|_| ResetError::new("无法读取 MySQL 当前账号权限"))?;
    let mut granted = BTreeSet::new();
    let mut all = false;
    for row in rows {
        let grant = String::try_get_by_index(&row, 0)
            .map_err(|_| ResetError::new("MySQL grant 返回格式无效"))?
            .to_ascii_uppercase();
        let Some((prefix, _)) = grant.split_once(" ON *.*") else {
            continue;
        };
        let list = prefix.strip_prefix("GRANT ").unwrap_or_default();
        if list == "ALL PRIVILEGES" {
            all = true;
            break;
        }
        granted.extend(list.split(',').map(str::trim).map(str::to_owned));
    }
    if !all
        && REQUIRED_GLOBAL_PRIVILEGES
            .iter()
            .any(|privilege| !granted.contains(*privilege))
    {
        return Err(ResetError::new(
            "MySQL 当前账号缺少重建所需的全局 DDL/DML 权限",
        ));
    }
    Ok(())
}

async fn inspect_database_ownership_on_server<C>(
    db: &C,
    resource: &PhysicalDatabase,
) -> ResetResult<OwnershipState>
where
    C: ConnectionTrait + ?Sized,
{
    let table_exists = scalar_i64(
        db,
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = ? AND table_name = 'ryframe_resource_ownership'",
        [resource.database.as_str().into()],
    )
    .await?
        == Some(1);
    if !table_exists {
        return Ok(OwnershipState::Missing);
    }
    let database = quote_identifier(&resource.database)?;
    let sql =
        format!("SELECT marker FROM {database}.ryframe_resource_ownership WHERE resource_kind = ?");
    for (kind, expected) in &resource.ownership_markers {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                &sql,
                [kind.as_str().into()],
            ))
            .await
            .map_err(|_| ResetError::new("无法读取 MySQL 所有权 marker"))?;
        let Some(row) = row else {
            return Ok(OwnershipState::Missing);
        };
        let actual = String::try_get_by_index(&row, 0)
            .map_err(|_| ResetError::new("MySQL 所有权 marker 格式无效"))?;
        if actual != *expected {
            return Ok(OwnershipState::Mismatch);
        }
    }
    Ok(OwnershipState::Verified)
}

async fn write_seed_hashes(db: &DatabaseConnection, seeds: &SeedCredentials) -> ResetResult<()> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "UPDATE sys_user SET password_hash = CASE username WHEN 'admin' THEN ? WHEN 'user' THEN ? ELSE password_hash END WHERE username IN ('admin', 'user')",
            [
                seeds.admin_hash.as_str().into(),
                seeds.user_hash.as_str().into(),
            ],
        ))
        .await
        .map_err(|_| ResetError::new("写入环境提供的种子密码失败"))?;
    if result.rows_affected() != 2 {
        return Err(ResetError::new("种子账号数量不符合唯一 baseline 预期"));
    }
    Ok(())
}

async fn verify_seed_hashes(db: &DatabaseConnection, seeds: &SeedCredentials) -> ResetResult<()> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT username, password_hash FROM sys_user WHERE username IN ('admin', 'user') ORDER BY username",
        ))
        .await
        .map_err(|_| ResetError::new("读取种子账号验证信息失败"))?;
    if rows.len() != 2 {
        return Err(ResetError::new("种子账号验证数量不匹配"));
    }
    for row in rows {
        let username = String::try_get_by_index(&row, 0)
            .map_err(|_| ResetError::new("种子账号验证格式无效"))?;
        let actual = String::try_get_by_index(&row, 1)
            .map_err(|_| ResetError::new("种子密码验证格式无效"))?;
        let password = match username.as_str() {
            "admin" => &seeds.admin_password,
            "user" => &seeds.user_password,
            _ => return Err(ResetError::new("出现未知种子账号")),
        };
        if !ryframe_auth::password::verify(password, &actual)
            .map_err(|_| ResetError::new("种子密码验证失败"))?
        {
            return Err(ResetError::new("环境种子密码未按预期持久化"));
        }
    }
    Ok(())
}

async fn verify_empty_control_resources(db: &DatabaseConnection) -> ResetResult<()> {
    for table in EMPTY_CONTROL_TABLES {
        let quoted = quote_identifier(table)?;
        let count = scalar_i64(db, &format!("SELECT COUNT(*) FROM {quoted}"), []).await?;
        if count != Some(0) {
            return Err(ResetError::new(format!(
                "控制库运行时资源表 {table} 在重建后非空"
            )));
        }
    }
    Ok(())
}

async fn verify_empty_external_tenant(db: &DatabaseConnection) -> ResetResult<()> {
    if scalar_i64(db, "SELECT COUNT(*) FROM biz_tenant_fence", []).await? != Some(0) {
        return Err(ResetError::new("外部租户库 fence 在重建后非空"));
    }
    let occupied = scalar_i64(
        db,
        "SELECT COUNT(*) FROM biz_tenant_target_slot WHERE tenant_id IS NOT NULL OR placement_generation IS NOT NULL OR switch_token IS NOT NULL",
        [],
    )
    .await?;
    if occupied != Some(0) {
        return Err(ResetError::new("外部租户库 slot 在重建后被占用"));
    }
    for table in ryframe_tenant_db::migration::TENANT_DATA_CATALOG.tables() {
        let quoted = quote_identifier(table.table)?;
        if scalar_i64(db, &format!("SELECT COUNT(*) FROM {quoted}"), []).await? != Some(0) {
            return Err(ResetError::new("外部租户业务表在重建后非空"));
        }
    }
    Ok(())
}

pub fn quote_identifier(identifier: &str) -> ResetResult<String> {
    if identifier.is_empty()
        || identifier.len() > 64
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(ResetError::new("拒绝引用不安全的 MySQL 标识符"));
    }
    Ok(format!("`{identifier}`"))
}

async fn scalar_i64<C, const N: usize>(
    db: &C,
    sql: &str,
    values: [sea_orm::Value; N],
) -> ResetResult<Option<i64>>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            sql,
            values,
        ))
        .await
        .map_err(|_| ResetError::new("MySQL 安全预检查询失败"))?;
    match row {
        Some(row) => Option::<i64>::try_get_by_index(&row, 0)
            .map_err(|_| ResetError::new("MySQL 标量查询格式无效")),
        None => Ok(None),
    }
}
