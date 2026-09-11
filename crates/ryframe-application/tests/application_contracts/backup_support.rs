use chrono::{DateTime, Duration, Utc};
use ryframe_application::{
    ports::backup::*,
    system::operations::{
        BACKUP_OBJECT_BUCKETS, BackupService, validate_backup_record, validate_restore_plan,
    },
};
use ryframe_kernel::{AppError, AppResult};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    },
};
use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Default)]
pub struct Records {
    pub backups: BTreeMap<String, BackupRecord>,
    pub restores: BTreeMap<String, RestoreRecord>,
}

pub struct Repository {
    pub records: Arc<Mutex<Records>>,
    pub clock: Arc<AtomicI64>,
}
struct Transaction {
    records: OwnedMutexGuard<Records>,
    clock: Arc<AtomicI64>,
    backups: Mutex<BTreeMap<String, BackupRecord>>,
    restores: Mutex<BTreeMap<String, RestoreRecord>>,
}

#[async_trait::async_trait]
impl BackupRepository for Repository {
    async fn database_now(&self) -> AppResult<DateTime<Utc>> {
        Ok(DateTime::from_timestamp(self.clock.load(Ordering::SeqCst), 0).unwrap())
    }
    async fn required_resources(&self) -> AppResult<Vec<String>> {
        Ok(manifest().resource_keys())
    }
    async fn begin(&self) -> AppResult<Box<dyn BackupTransaction>> {
        Ok(Box::new(Transaction {
            records: self.records.clone().lock_owned().await,
            clock: self.clock.clone(),
            backups: Mutex::default(),
            restores: Mutex::default(),
        }))
    }
    async fn backup(&self, id: &str) -> AppResult<Option<BackupRecord>> {
        Ok(self.records.lock().await.backups.get(id).cloned())
    }
    async fn restore(&self, id: &str) -> AppResult<Option<RestoreRecord>> {
        Ok(self.records.lock().await.restores.get(id).cloned())
    }
    async fn health(&self, _: &str, _: &[String], _: DateTime<Utc>) -> AppResult<BackupHealth> {
        Ok(BackupHealth::default())
    }
}

// 事务端口的写入在单独暂存区中完成，未提交时不能污染持久状态。
#[async_trait::async_trait]
impl BackupTransaction for Transaction {
    async fn save_backup(&self, record: &BackupRecord) -> AppResult<BackupRecord> {
        validate_backup_record(record)?;
        let mut staged = self.backups.lock().await;
        let existing = staged
            .get(&record.manifest.id)
            .or_else(|| self.records.backups.get(&record.manifest.id));
        if let Some(existing) = existing {
            validate_backup_record(existing)?;
            if existing.manifest != record.manifest
                || existing.manifest_hash != record.manifest_hash
            {
                return Err(AppError::Conflict("备份集 ID 已用于不同清单".into()));
            }
            if existing.checked_at >= record.checked_at {
                return Ok(existing.clone());
            }
        }
        staged.insert(record.manifest.id.clone(), record.clone());
        Ok(record.clone())
    }
    async fn create_restore(&self, record: &RestoreRecord) -> AppResult<RestoreRecord> {
        let mut staged = self.restores.lock().await;
        let existing = staged
            .get(&record.plan.id)
            .or_else(|| self.records.restores.get(&record.plan.id));
        if let Some(existing) = existing {
            if existing.plan != record.plan || existing.plan_hash != record.plan_hash {
                return Err(AppError::Conflict(
                    "恢复演练 ID 已用于不同的恢复计划".into(),
                ));
            }
            validate_restore_creation(record)?;
            validate_restore_record(existing)?;
            return Ok(existing.clone());
        }
        validate_restore_creation(record)?;
        let backup = {
            let staged = self.backups.lock().await;
            staged
                .get(&record.plan.backup_id)
                .or_else(|| self.records.backups.get(&record.plan.backup_id))
                .cloned()
        }
        .ok_or_else(|| AppError::NotFound("恢复演练绑定的备份集不存在".into()))?;
        let now = DateTime::from_timestamp(self.clock.load(Ordering::SeqCst), 0).unwrap();
        validate_restore_plan(&backup, &record.plan, now)?;
        if record.recovered_at != backup.manifest.captured_at {
            return Err(AppError::Validation(
                "恢复演练的实际恢复点与备份采集时间不一致".into(),
            ));
        }
        staged.insert(record.plan.id.clone(), record.clone());
        Ok(record.clone())
    }
    async fn advance_restore(
        &self,
        expected: &RestoreRecord,
        next: &RestoreRecord,
    ) -> AppResult<RestoreRecord> {
        validate_restore_advance(expected, next)?;
        let mut staged = self.restores.lock().await;
        let current = staged
            .get(&expected.plan.id)
            .or_else(|| self.records.restores.get(&expected.plan.id))
            .cloned()
            .ok_or_else(|| AppError::NotFound("恢复演练不存在".into()))?;
        if current != *expected {
            return Err(AppError::Conflict("恢复演练状态已变化".into()));
        }
        staged.insert(next.plan.id.clone(), next.clone());
        Ok(next.clone())
    }
    async fn commit(mut self: Box<Self>) -> AppResult<()> {
        let backups = std::mem::take(&mut *self.backups.lock().await);
        let restores = std::mem::take(&mut *self.restores.lock().await);
        self.records.backups.extend(backups);
        self.records.restores.extend(restores);
        Ok(())
    }
}

#[derive(Default)]
pub struct Verification {
    pub broken_file: AtomicBool,
    pub broken_data: AtomicBool,
    pub unavailable: AtomicBool,
    pub artifact_calls: AtomicU64,
    pub restored_database_calls: AtomicU64,
    pub restored_object_calls: AtomicU64,
    pub runtime_calls: AtomicU64,
    pub artifact_delay_seconds: AtomicI64,
    pub runtime_delay_seconds: AtomicI64,
    pub clock: Arc<AtomicI64>,
}

fn checked(failed: bool) -> AppResult<()> {
    if failed {
        Err(AppError::Validation("注入的校验失败".into()))
    } else {
        Ok(())
    }
}

#[async_trait::async_trait]
impl BackupArtifactVerifier for Verification {
    async fn artifacts(&self, _: &BackupManifest) -> AppResult<()> {
        self.artifact_calls.fetch_add(1, Ordering::SeqCst);
        self.clock.fetch_add(
            self.artifact_delay_seconds.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        checked(self.broken_file.load(Ordering::SeqCst))
    }
}
#[async_trait::async_trait]
impl BackupDatabaseVerifier for Verification {
    async fn snapshot(&self) -> AppResult<Vec<DatabaseBackup>> {
        Ok(manifest().databases)
    }
    async fn target_inventory(&self, key: &str) -> AppResult<DatabaseTargetInventory> {
        let database = manifest()
            .databases
            .into_iter()
            .find(|db| db.key == key)
            .ok_or_else(|| AppError::Validation("测试目标未登记".into()))?;
        Ok(DatabaseTargetInventory {
            database,
            preserved_tables: vec![],
        })
    }
    async fn validate_restore_targets(&self, _: &BackupManifest, _: &RestorePlan) -> AppResult<()> {
        checked(false)
    }
    async fn restored_databases(&self, _: &BackupManifest, _: &RestorePlan) -> AppResult<()> {
        self.restored_database_calls.fetch_add(1, Ordering::SeqCst);
        checked(self.broken_data.load(Ordering::SeqCst))
    }
}
#[async_trait::async_trait]
impl BackupObjectVerifier for Verification {
    async fn snapshot(&self) -> AppResult<Vec<ObjectBackup>> {
        Ok(manifest().objects)
    }
    async fn validate_restore_targets(&self, _: &RestorePlan) -> AppResult<()> {
        checked(false)
    }
    async fn restored_objects(&self, _: &BackupManifest, _: &RestorePlan) -> AppResult<()> {
        self.restored_object_calls.fetch_add(1, Ordering::SeqCst);
        checked(self.broken_data.load(Ordering::SeqCst))
    }
}
#[async_trait::async_trait]
impl BackupRuntimeVerifier for Verification {
    async fn restored_runtime(&self, _: &RestoreRecord, _: &RestoreBusinessProof) -> AppResult<()> {
        self.runtime_calls.fetch_add(1, Ordering::SeqCst);
        self.clock.fetch_add(
            self.runtime_delay_seconds.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        checked(self.unavailable.load(Ordering::SeqCst))
    }
}

pub fn fixture() -> (BackupService, Arc<Repository>, Arc<Verification>) {
    let clock = Arc::new(AtomicI64::new(1_700_000_000));
    let repository = Arc::new(Repository {
        records: Arc::default(),
        clock: clock.clone(),
    });
    let verifier = Arc::new(Verification {
        clock,
        ..Default::default()
    });
    let service = BackupService::new(
        repository.clone(),
        verifier.clone(),
        verifier.clone(),
        verifier.clone(),
        verifier.clone(),
    );
    (service, repository, verifier)
}

pub fn manifest() -> BackupManifest {
    let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let mut manifest = BackupManifest {
        id: "backup-1".into(),
        scope_id: "source".into(),
        source_sha: "a".repeat(40),
        quiesced_at: now - Duration::hours(2),
        captured_at: now - Duration::hours(1),
        completed_at: now - Duration::minutes(5),
        retention_until: now + Duration::days(7),
        control_schema_fingerprint: "a".repeat(16),
        tenant_schema_fingerprint: "b".repeat(64),
        databases: vec![DatabaseBackup {
            key: "shared-control".into(),
            kind: BackupDatabaseKind::Combined,
            server_uuid: "source-server".into(),
            database: "source_db".into(),
            shared: true,
            placements: vec![],
            tables: vec![BackupTableDigest {
                table: "sys_user".into(),
                rows: 42,
                sha256: "c".repeat(64),
            }],
        }],
        objects: BACKUP_OBJECT_BUCKETS
            .iter()
            .map(|bucket| ObjectBackup {
                bucket: (*bucket).into(),
                prefix: "source/".into(),
                entries: vec![],
            })
            .collect(),
        artifacts: vec![],
    };
    manifest.artifacts = manifest
        .resource_keys()
        .into_iter()
        .enumerate()
        .map(|(i, resource)| BackupArtifact {
            resource,
            relative_path: format!("backup-{i}.bin"),
            bytes: 42,
            sha256: "d".repeat(64),
        })
        .collect();
    manifest
}

pub fn plan() -> RestorePlan {
    RestorePlan {
        id: "drill-1".into(),
        backup_id: "backup-1".into(),
        scope_id: "restore-1".into(),
        fault_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        databases: vec![RestoreDatabase {
            source_key: "shared-control".into(),
            target_key: "shared-control".into(),
            server_uuid: "source-server".into(),
            database: "restore_db".into(),
        }],
        object_endpoint: "127.0.0.1:9000".into(),
        object_prefix: "restore-1/".into(),
        api_ready_url: "http://127.0.0.1:8081/readyz".into(),
        worker_ready_url: "http://127.0.0.1:9091/readyz".into(),
        frontend_sha: "e".repeat(40),
    }
}

pub fn proof(record: &RestoreRecord) -> RestoreBusinessProof {
    RestoreBusinessProof {
        restore_id: record.plan.id.clone(),
        plan_hash: record.plan_hash.clone(),
        backup_source_sha: "a".repeat(40),
        backend_product_sha: "b".repeat(40),
        backend_execution_sha: "b".repeat(40),
        backend_adapter_contract: None,
        frontend_sha: record.plan.frontend_sha.clone(),
        runner_sha: "e".repeat(40),
        verifier_sha: "c".repeat(40),
        scope_id: record.plan.scope_id.clone(),
        frontend_url: "http://127.0.0.1:4174".into(),
        runtime_receipt_sha256: "f".repeat(64),
        tests_receipt_sha256: "d".repeat(64),
        target_plan_sha256: "e".repeat(64),
        started_at: record.data_verified_at.unwrap(),
        completed_at: record.data_verified_at.unwrap(),
        scenarios: [
            "login",
            "session",
            "post",
            "notice",
            "tenant",
            "export",
            "message",
            "schedule",
            "restored-data",
        ]
        .map(|name| RestoreScenarioResult {
            name: name.into(),
            succeeded: true,
        })
        .to_vec(),
        unexpected_console_messages: 0,
        unexpected_network_failures: 0,
        axe_serious_or_critical: 0,
    }
}
