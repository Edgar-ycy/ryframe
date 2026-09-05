//! 外部备份登记、隔离恢复验证与运行状态端口。

use chrono::{DateTime, Utc};
use ryframe_kernel::AppResult;
use serde::{Deserialize, Serialize};

mod health;
mod manifest;
mod restore;

pub use health::*;
pub use manifest::*;
pub use restore::*;

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
    async fn restore(&self, id: &str) -> AppResult<Option<RestoreRecord>>;
    async fn save_restore(&self, record: &RestoreRecord) -> AppResult<()>;
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
