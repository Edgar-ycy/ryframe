use std::collections::BTreeSet;

use ryframe_config::{AppConfig, DbConnection};
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, TransactionTrait};

use crate::reset::{
    ResetError, ResetResult,
    engine::{PhaseEvidence, ResourceProgress},
    ledger::ResetLedger,
    model::{PhysicalDatabase, ResetManifest, database_resource_key, sha256_hex},
};

mod identity;
mod operations;

use identity::{
    collect_specs, database_physical_identity, load_seed_credentials, server_identity,
    validate_physical_database_identities,
};
use operations::{
    connect_target, database_exists, scalar_i64, verify_database_ownership_before_recreate,
    verify_empty_control_resources, verify_empty_external_tenant, verify_global_privileges,
    verify_seed_hashes, verify_server_writable, write_seed_hashes,
};

pub use identity::{parse_lower_case_table_names, same_credentials};
pub use operations::quote_identifier;

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

pub(super) struct DatabaseHandle {
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

pub(super) struct SeedCredentials {
    admin_password: String,
    admin_hash: String,
    user_password: String,
    user_hash: String,
}

#[derive(Clone)]
pub(super) struct DatabaseSpec {
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
                ryframe_tenant_db::migration::tenant_data_schema_fingerprint().into(),
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
