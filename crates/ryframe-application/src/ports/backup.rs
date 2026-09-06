//! 外部备份登记、隔离恢复验证与运行状态端口。

use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};

mod health;
mod manifest;
mod restore;

pub use health::*;
pub use manifest::*;
pub use restore::*;

/// 计算备份与恢复契约使用的规范 JSON 内容摘要。
pub fn backup_content_hash(value: &impl Serialize) -> AppResult<String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(value)
        .map_err(|_| AppError::Validation("备份或演练清单无法序列化".into()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub(crate) fn has_storage_timestamp_precision(value: DateTime<Utc>) -> bool {
    value.timestamp_subsec_nanos().is_multiple_of(1_000)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackupRecord {
    pub manifest: BackupManifest,
    pub manifest_hash: String,
    pub valid: bool,
    pub checked_at: DateTime<Utc>,
    pub failure: Option<String>,
}

#[async_trait::async_trait]
pub trait BackupTransaction: Send + Sync {
    async fn backup(&self, id: &str) -> AppResult<Option<BackupRecord>>;
    async fn save_backup(&self, record: &BackupRecord) -> AppResult<()>;
    /// 原子创建恢复演练。相同 ID 与完整计划的重试返回首次持久化的权威记录；
    /// 相同 ID 绑定不同计划或计划摘要时返回冲突。
    /// 实现必须先调用 [`validate_restore_creation`] 拒绝无效初始记录。
    async fn create_restore(&self, record: &RestoreRecord) -> AppResult<RestoreRecord>;
    /// 按完整旧记录执行一次状态 CAS，并返回持久化后的权威记录。
    /// 实现必须保持计划、计划摘要、开始时间与恢复点时间不可变，并拒绝非法状态转换。
    /// 实现必须调用 [`validate_restore_advance`] 校验旧记录和下一记录的完整形状。
    async fn advance_restore(
        &self,
        expected: &RestoreRecord,
        next: &RestoreRecord,
    ) -> AppResult<RestoreRecord>;
    async fn commit(self: Box<Self>) -> AppResult<()>;
}

#[async_trait::async_trait]
pub trait BackupRepository: Send + Sync {
    async fn database_now(&self) -> AppResult<DateTime<Utc>>;
    async fn required_resources(&self) -> AppResult<Vec<String>>;
    async fn begin(&self) -> AppResult<Box<dyn BackupTransaction>>;
    async fn backup(&self, id: &str) -> AppResult<Option<BackupRecord>>;
    async fn restore(&self, id: &str) -> AppResult<Option<RestoreRecord>>;
    async fn health(
        &self,
        scope_id: &str,
        resources: &[String],
        now: DateTime<Utc>,
    ) -> AppResult<BackupHealth>;
}

#[async_trait::async_trait]
pub trait BackupArtifactVerifier: Send + Sync {
    /// 验证清单中每个外部文件的长度、SHA-256 和受限相对路径。
    async fn artifacts(&self, manifest: &BackupManifest) -> AppResult<()>;
}

#[async_trait::async_trait]
pub trait BackupDatabaseVerifier: Send + Sync {
    async fn snapshot(&self) -> AppResult<Vec<DatabaseBackup>>;
    /// 只读取明确配置的目标，包括尚未分配租户的目标，不发现其他数据库。
    async fn target_inventory(&self, key: &str) -> AppResult<DatabaseTargetInventory>;
    async fn validate_restore_targets(
        &self,
        manifest: &BackupManifest,
        plan: &RestorePlan,
    ) -> AppResult<()>;
    /// 只读验证已登记隔离目标的 ownership、schema 与完整数据。
    async fn restored_databases(
        &self,
        manifest: &BackupManifest,
        plan: &RestorePlan,
    ) -> AppResult<()>;
}

#[async_trait::async_trait]
pub trait BackupObjectVerifier: Send + Sync {
    async fn snapshot(&self) -> AppResult<Vec<ObjectBackup>>;
    async fn validate_restore_targets(&self, plan: &RestorePlan) -> AppResult<()>;
    async fn restored_objects(
        &self,
        manifest: &BackupManifest,
        plan: &RestorePlan,
    ) -> AppResult<()>;
}

#[async_trait::async_trait]
pub trait BackupRuntimeVerifier: Send + Sync {
    /// 验证实际就绪端点与同一次隔离演练产生的业务验收证据。
    async fn restored_runtime(
        &self,
        record: &RestoreRecord,
        proof: &RestoreBusinessProof,
    ) -> AppResult<()>;
}
