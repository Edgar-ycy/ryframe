use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 外部工具完成一致性备份后提交的清单；不包含凭据或数据库连接串。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    pub id: String,
    pub scope_id: String,
    pub source_sha: String,
    pub quiesced_at: DateTime<Utc>,
    pub captured_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub retention_until: DateTime<Utc>,
    pub control_schema_fingerprint: String,
    pub tenant_schema_fingerprint: String,
    pub databases: Vec<DatabaseBackup>,
    pub objects: Vec<ObjectBackup>,
    pub artifacts: Vec<BackupArtifact>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupDatabaseKind {
    Control,
    Tenant,
    Combined,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseBackup {
    pub key: String,
    pub kind: BackupDatabaseKind,
    pub server_uuid: String,
    pub database: String,
    pub shared: bool,
    pub placements: Vec<BackupPlacement>,
    pub tables: Vec<BackupTableDigest>,
}

/// 显式目标的完整只读清单；登记表与迁移、ownership 不作为业务恢复数据复制。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseTargetInventory {
    pub database: DatabaseBackup,
    pub preserved_tables: Vec<BackupTableDigest>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackupPlacement {
    pub tenant_id: String,
    pub generation: i64,
    pub switch_token: String,
}

/// 按主键顺序、列顺序及明确的 NULL 编码计算完整表的 SHA-256。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackupTableDigest {
    pub table: String,
    pub rows: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectBackup {
    pub bucket: String,
    pub prefix: String,
    pub entries: Vec<BackupObjectDigest>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackupObjectDigest {
    pub key: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackupArtifact {
    /// 必须与清单中的 db:<key> 或 objects:<bucket> 完全对应。
    pub resource: String,
    pub relative_path: String,
    pub bytes: u64,
    pub sha256: String,
}

impl BackupManifest {
    pub fn resource_keys(&self) -> Vec<String> {
        self.databases
            .iter()
            .map(|db| format!("db:{}", db.key))
            .chain(
                self.objects
                    .iter()
                    .map(|item| format!("objects:{}", item.bucket)),
            )
            .collect()
    }
}
