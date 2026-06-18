use chrono::{DateTime, Utc};
use ryframe_macro::AutoFill;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, AutoFill)]
#[sea_orm(table_name = "password_reset_requests")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[auto_fill(snowflake)]
    pub id: i64,
    pub target_user_id: i64,
    pub requested_by: i64,
    pub reason: String,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub request_ip: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const STATUS_PENDING: &str = "pending";
    pub const STATUS_COMPLETED: &str = "completed";
    pub const STATUS_EXPIRED: &str = "expired";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
