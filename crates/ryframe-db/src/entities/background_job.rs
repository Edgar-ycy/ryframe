use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

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
    pub scheduled_for: Option<DateTime<Utc>>,
    pub max_runtime_seconds: Option<i32>,
    pub job_type: String,
    pub payload: Json,
    pub status: String,
    pub priority: i32,
    pub available_at: DateTime<Utc>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTime<Utc>>,
    pub dedupe_key: Option<String>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
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
