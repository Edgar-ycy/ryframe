use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 生成或上传的租户配置包。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_tenant_config_bundle")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub origin: String,
    pub source_tenant_key: String,
    pub source_tenant_name_snapshot: String,
    pub package_schema_version: String,
    pub source_app_version: String,
    pub file_id: Option<i64>,
    pub sha256: Option<String>,
    pub resource_counts: Json,
    pub item_count: i32,
    pub status: String,
    pub background_job_id: Option<i64>,
    pub idempotency_key_hash: Option<String>,
    pub created_by: i64,
    pub error_summary: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const ORIGIN_GENERATED: &str = "generated";
    pub const ORIGIN_UPLOADED: &str = "uploaded";
    pub const STATUS_PENDING: &str = "pending";
    pub const STATUS_RUNNING: &str = "running";
    pub const STATUS_SUCCEEDED: &str = "succeeded";
    pub const STATUS_FAILED: &str = "failed";
    pub const STATUS_EXPIRED: &str = "expired";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
