//! 租户数据迁移实体。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// 一次租户业务数据停写迁移的权威状态。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_tenant_data_migration")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub source_target_key: String,
    pub target_key: String,
    pub source_target_mode: String,
    pub source_target_kind: String,
    pub target_target_mode: String,
    pub target_target_kind: String,
    pub source_generation: i64,
    pub source_switch_token: String,
    pub target_generation: i64,
    pub source_schema_fingerprint: String,
    pub target_schema_fingerprint: String,
    pub plan_hash: String,
    pub create_idempotency_key_hash: String,
    pub cancel_idempotency_key_hash: Option<String>,
    pub finalize_idempotency_key_hash: Option<String>,
    pub state: String,
    pub switch_token: String,
    pub operator_id: i64,
    pub cancelled_by: Option<i64>,
    pub finalized_by: Option<i64>,
    pub background_job_id: Option<i64>,
    pub retention_hours: i32,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub prechecked_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub queued_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub quiesced_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub frozen_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub copy_started_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub copy_completed_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub verified_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub cut_over_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub activated_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub succeeded_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub retention_until: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub cancel_requested_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub finalize_requested_at: Option<OffsetDateTime>,
    /// catalog 批次均已清空、可安全移除目标 fence/slot 的控制面检查点。
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub cleanup_ready_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub finalized_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub failed_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub cancelled_at: Option<OffsetDateTime>,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub created_at: OffsetDateTime,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub updated_at: OffsetDateTime,
}

impl Model {
    pub const STATE_PRECHECKING: &str = "prechecking";
    pub const STATE_QUEUED: &str = "queued";
    pub const STATE_QUIESCING: &str = "quiescing";
    pub const STATE_FROZEN: &str = "frozen";
    pub const STATE_COPYING: &str = "copying";
    pub const STATE_VERIFYING: &str = "verifying";
    pub const STATE_CUTTING_OVER: &str = "cutting_over";
    pub const STATE_ACTIVATING: &str = "activating";
    pub const STATE_SUCCEEDED: &str = "succeeded";
    pub const STATE_RETENTION_PENDING: &str = "retention_pending";
    pub const STATE_FINALIZED: &str = "finalized";
    pub const STATE_FAILED: &str = "failed";
    pub const STATE_CANCELLED: &str = "cancelled";

    pub fn can_cancel(&self) -> bool {
        matches!(
            self.state.as_str(),
            Self::STATE_PRECHECKING
                | Self::STATE_QUEUED
                | Self::STATE_QUIESCING
                | Self::STATE_FROZEN
                | Self::STATE_COPYING
                | Self::STATE_VERIFYING
        )
    }

    pub fn can_finalize(&self) -> bool {
        self.state == Self::STATE_RETENTION_PENDING
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
