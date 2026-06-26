use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_tenant")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
    /// Increased whenever tenant sessions must be invalidated.
    pub session_version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const STATUS_DISABLED: &str = "0";
    pub const STATUS_NORMAL: &str = "1";

    pub fn is_available(&self, now: DateTime<Utc>) -> bool {
        self.status == Self::STATUS_NORMAL && self.expire_at.is_none_or(|expire_at| expire_at > now)
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
