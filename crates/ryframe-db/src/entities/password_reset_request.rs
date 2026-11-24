use ryframe_kernel::AppResult;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::auto_fill::{AutoFill, FillContext};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "password_reset_requests")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub target_user_id: i64,
    pub requested_by: i64,
    pub reason: String,
    pub token_hash: String,
    pub expires_at: PrimitiveDateTime,
    pub completed_at: Option<PrimitiveDateTime>,
    pub request_ip: Option<String>,
    pub status: String,
    pub created_at: PrimitiveDateTime,
    pub updated_at: PrimitiveDateTime,
}

impl Model {
    pub const STATUS_PENDING: &str = "pending";
    pub const STATUS_COMPLETED: &str = "completed";
    pub const STATUS_EXPIRED: &str = "expired";
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

pub(crate) fn utc_datetime(instant: OffsetDateTime) -> PrimitiveDateTime {
    let utc = instant.to_utc();
    PrimitiveDateTime::new(utc.date(), utc.time())
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
