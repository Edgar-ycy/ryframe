use chrono::{DateTime, Utc};
use ryframe_macro::AutoFill;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, AutoFill)]
#[sea_orm(table_name = "sys_file")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[auto_fill(snowflake)]
    pub id: i64,
    pub tenant_id: String,
    pub original_name: String,
    pub storage_name: String,
    pub storage_path: String,
    pub bucket: String,
    pub file_url: String,
    pub file_size: i64,
    pub content_type: String,
    pub file_md5: Option<String>,
    pub upload_by: Option<String>,
    pub upload_status: String,
    pub reservation_token: Option<String>,
    pub reservation_expires_at: Option<DateTime<Utc>>,
    pub del_flag: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const UPLOAD_STATUS_PENDING: &str = "pending";
    pub const UPLOAD_STATUS_READY: &str = "ready";
    pub const UPLOAD_STATUS_CLEANUP: &str = "cleanup";

    pub const DEL_FLAG_NORMAL: &str = "0";
    pub const DEL_FLAG_DELETED: &str = "2";
    /// Upload reservations use a value unknown to the previous release, whose
    /// readers only expose `del_flag = '0'`. This keeps pending/cleanup rows
    /// invisible during rolling upgrades without colliding with soft delete.
    pub const DEL_FLAG_UPLOAD_RESERVED: &str = "3";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
