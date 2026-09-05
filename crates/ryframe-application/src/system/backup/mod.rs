//! 外部备份与恢复演练用例。恢复计时从登记开始，包含外部恢复及业务验收。

use crate::ports::backup::*;
use ryframe_kernel::{AppError, AppResult};
use std::sync::Arc;

mod validation;
pub use validation::{validate_backup_manifest, validate_restore_plan, validate_restore_proof};

pub const BACKUP_OBJECT_BUCKETS: &[&str] = &[
    crate::system::content::UPLOAD_BUCKET,
    crate::system::content::AVATAR_BUCKET,
    crate::system::operations::EXPORT_BUCKET,
    crate::system::content::IMPORT_BUCKET,
    crate::system::content::CONFIG_PACKAGE_BUCKET,
];

pub struct BackupService {
    repository: Arc<dyn BackupRepository>,
    artifacts: Arc<dyn BackupArtifactVerifier>,
    databases: Arc<dyn BackupDatabaseVerifier>,
    objects: Arc<dyn BackupObjectVerifier>,
    runtime: Arc<dyn BackupRuntimeVerifier>,
}

impl BackupService {
    pub fn new(
        repository: Arc<dyn BackupRepository>,
        artifacts: Arc<dyn BackupArtifactVerifier>,
        databases: Arc<dyn BackupDatabaseVerifier>,
        objects: Arc<dyn BackupObjectVerifier>,
        runtime: Arc<dyn BackupRuntimeVerifier>,
    ) -> Self {
        Self {
            repository,
            artifacts,
            databases,
            objects,
            runtime,
        }
    }

    pub async fn register(
        &self,
        manifest: BackupManifest,
        required: &[String],
    ) -> AppResult<BackupRecord> {
        let now = self.repository.database_now().await?;
        validate_backup_manifest(&manifest, required, now)?;
        let manifest_hash = backup_content_hash(&manifest)?;
        let verification = self.artifacts.artifacts(&manifest).await;
        let record = BackupRecord {
            manifest,
            manifest_hash,
            valid: verification.is_ok(),
            checked_at: self.repository.database_now().await?,
            failure: verification.as_ref().err().map(failure_detail),
        };
        let transaction = self.repository.begin().await?;
        if transaction
            .backup(&record.manifest.id)
            .await?
            .is_some_and(|existing| existing.manifest_hash != record.manifest_hash)
        {
            return Err(AppError::Conflict("备份集 ID 已用于不同清单".into()));
        }
        transaction.save_backup(&record).await?;
        transaction.commit().await?;
        verification?;
        Ok(record)
    }

    pub async fn begin_restore(&self, plan: RestorePlan) -> AppResult<RestoreRecord> {
        let now = self.repository.database_now().await?;
        let backup = self.require_backup(&plan.backup_id).await?;
        validate_restore_plan(&backup, &plan, now)?;
        self.databases
            .validate_restore_targets(&backup.manifest, &plan)
            .await?;
        self.objects.validate_restore_targets(&plan).await?;
        let plan_hash = backup_content_hash(&plan)?;
        let transaction = self.repository.begin().await?;
        if let Some(existing) = transaction.restore(&plan.id).await? {
            if existing.plan_hash != plan_hash {
                return Err(AppError::Conflict("恢复演练 ID 已用于其他目标".into()));
            }
            return Ok(existing);
        }
        let record = RestoreRecord {
            plan,
            plan_hash,
            status: RestoreStatus::Running,
            started_at: now,
            data_verified_at: None,
            completed_at: None,
            recovered_at: backup.manifest.captured_at,
            failure: None,
        };
        transaction.save_restore(&record).await?;
        transaction.commit().await?;
        Ok(record)
    }

    pub async fn verify_data(&self, id: &str) -> AppResult<RestoreRecord> {
        let record = self.require_restore(id).await?;
        if record.status != RestoreStatus::Running {
            return Err(AppError::Conflict("仅运行中的演练可以验证恢复数据".into()));
        }
        let backup = self.require_backup(&record.plan.backup_id).await?;
        let result = self.artifacts.artifacts(&backup.manifest).await;
        let result = match result {
            Ok(()) => {
                self.databases
                    .restored_databases(&backup.manifest, &record.plan)
                    .await
            }
            Err(error) => Err(error),
        };
        let result = match result {
            Ok(()) => {
                self.objects
                    .restored_objects(&backup.manifest, &record.plan)
                    .await
            }
            Err(error) => Err(error),
        };
        self.advance_restore(record, RestoreStatus::DataVerified, result)
            .await
    }

    pub async fn finish_restore(
        &self,
        id: &str,
        proof: &RestoreBusinessProof,
    ) -> AppResult<RestoreRecord> {
        let record = self.require_restore(id).await?;
        if record.status != RestoreStatus::DataVerified {
            return Err(AppError::Conflict(
                "恢复数据验证通过后才能进行运行与业务验收".into(),
            ));
        }
        let backup = self.require_backup(&record.plan.backup_id).await?;
        let now = self.repository.database_now().await?;
        let result = match validate_restore_proof(&backup, &record, proof, now) {
            Ok(()) => self.runtime.restored_runtime(&record, proof).await,
            Err(error) => Err(error),
        };
        self.advance_restore(record, RestoreStatus::Succeeded, result)
            .await
    }

    async fn advance_restore(
        &self,
        previous: RestoreRecord,
        next: RestoreStatus,
        result: AppResult<()>,
    ) -> AppResult<RestoreRecord> {
        let now = self.repository.database_now().await?;
        let result = result.and_then(|()| {
            if now < previous.started_at || (now - previous.started_at).num_seconds() > 3600 {
                Err(AppError::Validation(
                    "恢复演练超过 60 分钟或时钟发生回退".into(),
                ))
            } else {
                Ok(())
            }
        });
        let transaction = self.repository.begin().await?;
        let mut record = transaction
            .restore(&previous.plan.id)
            .await?
            .ok_or_else(|| AppError::NotFound("恢复演练不存在".into()))?;
        if record.status != previous.status || record.plan_hash != previous.plan_hash {
            return Err(AppError::Conflict("恢复演练状态已变化".into()));
        }
        record.status = if result.is_ok() {
            next
        } else {
            RestoreStatus::Failed
        };
        record.failure = result.as_ref().err().map(failure_detail);
        if record.status == RestoreStatus::DataVerified {
            record.data_verified_at = Some(now);
        } else {
            record.completed_at = Some(now.max(record.started_at));
        }
        transaction.save_restore(&record).await?;
        transaction.commit().await?;
        result?;
        Ok(record)
    }

    async fn require_backup(&self, id: &str) -> AppResult<BackupRecord> {
        self.repository
            .backup(id)
            .await?
            .filter(|record| record.valid)
            .ok_or_else(|| AppError::NotFound("未找到有效备份集".into()))
    }

    async fn require_restore(&self, id: &str) -> AppResult<RestoreRecord> {
        self.repository
            .restore(id)
            .await?
            .ok_or_else(|| AppError::NotFound("恢复演练不存在".into()))
    }
}

pub fn backup_content_hash(value: &impl serde::Serialize) -> AppResult<String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(value)
        .map_err(|_| AppError::Validation("备份或演练清单无法序列化".into()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn failure_detail(error: &AppError) -> String {
    error.to_string().chars().take(1000).collect()
}
