use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 配置迁移预览和执行的逐项结果。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_tenant_config_transfer_item")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub transfer_id: i64,
    pub resource_type: String,
    pub stable_key: String,
    pub display_name: String,
    pub action: String,
    pub outcome: String,
    pub detail_code: Option<String>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const ACTION_CREATE: &str = "create";
    pub const ACTION_UPDATE: &str = "update";
    pub const ACTION_UNCHANGED: &str = "unchanged";
    pub const ACTION_CONFLICT: &str = "conflict";
    pub const ACTION_BLOCKED: &str = "blocked";
    pub const OUTCOME_PENDING: &str = "pending";
    pub const OUTCOME_APPLIED: &str = "applied";
    pub const OUTCOME_SKIPPED: &str = "skipped";
    pub const OUTCOME_FAILED: &str = "failed";
    pub const OUTCOME_ROLLED_BACK: &str = "rolled_back";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
