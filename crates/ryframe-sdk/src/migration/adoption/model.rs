use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::BusinessMigrationScope;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusinessAdoptionTarget {
    pub server_uuid: String,
    pub database: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusinessAdoptionLedger {
    pub existed: bool,
    pub fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusinessAdoptionTable {
    pub table: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusinessAdoptionMigration {
    pub id: String,
    pub checksum: String,
    pub before_fingerprint: Option<String>,
    pub after_fingerprint: String,
    pub tables: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusinessAdoptionManifest {
    pub version: u16,
    pub scope: BusinessMigrationScope,
    pub module: String,
    pub target: BusinessAdoptionTarget,
    pub ledger: BusinessAdoptionLedger,
    pub migrations: Vec<BusinessAdoptionMigration>,
    pub tables: Vec<BusinessAdoptionTable>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusinessAdoptionPlan {
    pub manifest: BusinessAdoptionManifest,
    pub fingerprint: String,
}

impl BusinessAdoptionPlan {
    pub(super) fn new(manifest: BusinessAdoptionManifest) -> AppResult<Self> {
        let fingerprint = fingerprint(&manifest)?;
        Ok(Self {
            manifest,
            fingerprint,
        })
    }

    pub(super) fn validate(&self) -> AppResult<()> {
        if self.manifest.version != 1 || self.fingerprint != fingerprint(&self.manifest)? {
            return Err(AppError::Config(
                "接管计划版本或摘要不一致；重新生成并审阅计划，不修改数据库".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BusinessAdoptionState {
    Unapplied,
    Applied,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BusinessAdoptionOutcome {
    pub already_applied: bool,
    pub registered: usize,
    pub metadata_ddl_attempted: bool,
}

pub(super) fn fingerprint<T: Serialize + ?Sized>(value: &T) -> AppResult<String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| AppError::Config(format!("无法序列化接管证据：{error}")))?;
    Ok(digest_hex(&bytes))
}

pub(super) fn schema_fingerprint(value: &str) -> String {
    digest_hex(value.as_bytes())
}

fn digest_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
