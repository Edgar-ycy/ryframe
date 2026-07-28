use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 与业务数据同事务提交的异步事件。
///
/// 事件在提交后由 Worker 至少一次投递；消费者必须以事件 ID 或幂等键保证重复
/// 投递不会产生重复副作用。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_outbox_event")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: Option<String>,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub payload: Json,
    pub status: String,
    pub available_at: DateTime<Utc>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTime<Utc>>,
    pub dedupe_key: Option<String>,
    pub traceparent: Option<String>,
    pub last_error: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const STATUS_PENDING: &str = "pending";
    pub const STATUS_RUNNING: &str = "running";
    pub const STATUS_PUBLISHED: &str = "published";
    pub const STATUS_DEAD: &str = "dead";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
