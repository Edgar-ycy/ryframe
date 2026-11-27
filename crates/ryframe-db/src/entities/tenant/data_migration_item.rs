//! 租户数据迁移明细实体。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// 停写迁移中单张编译期 catalog 表的复制和校验检查点。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_tenant_data_migration_item")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub migration_id: i64,
    pub table_name: String,
    pub copy_order: i32,
    pub state: String,
    pub cursor_json: Option<Json>,
    pub source_row_count: Option<i64>,
    pub target_row_count: Option<i64>,
    pub source_digest: Option<String>,
    pub target_digest: Option<String>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
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
    pub copied_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub verified_at: Option<OffsetDateTime>,
    pub cleanup_state: String,
    pub cleanup_row_count: i64,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub created_at: OffsetDateTime,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub updated_at: OffsetDateTime,
}

impl Model {
    pub const STATE_PENDING: &str = "pending";
    pub const STATE_COPYING: &str = "copying";
    pub const STATE_COPIED: &str = "copied";
    pub const STATE_VERIFYING: &str = "verifying";
    pub const STATE_VERIFIED: &str = "verified";
    pub const STATE_FAILED: &str = "failed";
    pub const CLEANUP_PENDING: &str = "pending";
    pub const CLEANUP_CLEANING: &str = "cleaning";
    pub const CLEANUP_CLEANED: &str = "cleaned";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
