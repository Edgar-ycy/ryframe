use ryframe_kernel::AppResult;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::auto_fill::{AutoFill, FillContext};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_file")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub original_name: String,
    pub storage_name: String,
    pub storage_path: String,
    pub bucket: String,
    pub file_url: String,
    pub file_size: i64,
    pub content_type: String,
    /// SHA-256 是正常运行时唯一的权威文件摘要。
    pub file_sha256: String,
    pub upload_by: Option<String>,
    pub upload_status: String,
    pub reservation_token: Option<String>,
    pub reservation_expires_at: Option<PrimitiveDateTime>,
    pub del_flag: String,
    pub created_at: PrimitiveDateTime,
    pub updated_at: PrimitiveDateTime,
}

impl Model {
    pub const UPLOAD_STATUS_PENDING: &str = "pending";
    pub const UPLOAD_STATUS_READY: &str = "ready";
    pub const UPLOAD_STATUS_CLEANUP: &str = "cleanup";

    pub const DEL_FLAG_NORMAL: &str = "0";
    pub const DEL_FLAG_DELETED: &str = "2";
}

impl AutoFill for Model {
    fn fill_on_insert(&mut self, context: &FillContext) -> AppResult<()> {
        let now = context.sql_utc_datetime()?;
        self.created_at = now;
        self.updated_at = now;
        self.id = crate::next_id()?;
        Ok(())
    }

    fn fill_on_update(&mut self, context: &FillContext) -> AppResult<()> {
        self.updated_at = context.sql_utc_datetime()?;
        Ok(())
    }
}

/// 将应用层的绝对时刻投影为 MySQL DATETIME 中保存的 UTC 值。
pub(crate) fn utc_datetime(instant: OffsetDateTime) -> PrimitiveDateTime {
    let utc = instant.to_utc();
    PrimitiveDateTime::new(utc.date(), utc.time())
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
