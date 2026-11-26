use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// 可持久化的异步任务单元。
///
/// 处理器执行前会先领取记录，因此 Worker 提供至少一次投递。`lease_owner` 和
/// `lease_until` 是所有权令牌，而非提示性元数据：完成或失败更新必须比对它们，
/// 以免过期 Worker 完成已重新租约的任务。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_background_job")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: Option<String>,
    pub schedule_id: Option<i64>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub scheduled_for: Option<OffsetDateTime>,
    pub max_runtime_seconds: Option<i32>,
    pub job_type: String,
    pub payload: Json,
    pub status: String,
    pub priority: i32,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub available_at: OffsetDateTime,
    pub attempts: i32,
    /// 每次领取递增，不随重试预算或人工重试重置。
    pub claim_sequence: i64,
    pub max_attempts: i32,
    pub lease_owner: Option<String>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub lease_until: Option<OffsetDateTime>,
    pub dedupe_key: Option<String>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
    pub last_error: Option<String>,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub created_at: OffsetDateTime,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub updated_at: OffsetDateTime,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub completed_at: Option<OffsetDateTime>,
}

impl Model {
    pub const STATUS_PENDING: &str = "pending";
    pub const STATUS_RUNNING: &str = "running";
    pub const STATUS_SUCCEEDED: &str = "succeeded";
    pub const STATUS_DEAD: &str = "dead";

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            Self::STATUS_SUCCEEDED | Self::STATUS_DEAD
        )
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
