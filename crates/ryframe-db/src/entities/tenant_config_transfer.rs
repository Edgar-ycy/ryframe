use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 一次面向目标租户的配置预览、应用或回滚。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_tenant_config_transfer")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub bundle_id: i64,
    pub idempotency_key_hash: String,
    pub request_kind: String,
    pub request_fingerprint: String,
    pub status: String,
    pub target_configuration_version: i64,
    pub target_authorization_epoch: i32,
    pub plan_hash: Option<String>,
    pub preview_calculated_at: Option<DateTime<Utc>>,
    pub preview_background_job_id: Option<i64>,
    pub apply_background_job_id: Option<i64>,
    pub rollback_background_job_id: Option<i64>,
    pub snapshot_file_id: Option<i64>,
    pub applied_configuration_version: Option<i64>,
    pub applied_authorization_epoch: Option<i32>,
    pub change_counts: Json,
    pub error_summary: Option<String>,
    pub requested_by: i64,
    pub rollback_expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const STATUS_PREVIEW_READY: &str = "preview_ready";
    pub const STATUS_PREVIEW_PENDING: &str = "preview_pending";
    pub const STATUS_PREVIEWING: &str = "previewing";
    pub const STATUS_PREVIEWED: &str = "previewed";
    pub const STATUS_APPLY_PENDING: &str = "apply_pending";
    pub const STATUS_APPLYING: &str = "applying";
    pub const STATUS_APPLIED: &str = "applied";
    pub const STATUS_ROLLBACK_PENDING: &str = "rollback_pending";
    pub const STATUS_ROLLING_BACK: &str = "rolling_back";
    pub const STATUS_ROLLED_BACK: &str = "rolled_back";
    pub const STATUS_FAILED: &str = "failed";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
