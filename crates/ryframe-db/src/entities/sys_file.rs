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
    /// 对 v0.6 及以后写入的文件，SHA-256 是权威摘要。迁移历史记录期间，旧 MD5
    /// 列仍可为空。
    pub file_sha256: Option<String>,
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
    /// 上传预留使用旧版本未知的值；旧版本读取器只暴露 `del_flag = '0'`。这使
    /// pending/cleanup 记录在滚动升级期间保持不可见，并且不会与软删除冲突。
    pub const DEL_FLAG_UPLOAD_RESERVED: &str = "3";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
