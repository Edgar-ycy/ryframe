use ryframe_application::ports::utc_timestamp::from_chrono;
use ryframe_kernel::AppResult;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{
    DbResultExt,
    auto_fill::{AutoFill, FillContext},
};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_config")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub name: String,
    #[sea_orm(unique)]
    pub key: String,
    pub value: String,
    pub portable: bool,
    pub remark: Option<String>,
    pub del_flag: String,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub created_at: OffsetDateTime,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub updated_at: OffsetDateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl Model {
    pub const DEL_FLAG_NORMAL: &str = "0";
    pub const DEL_FLAG_DELETED: &str = "2";
}

impl AutoFill for Model {
    fn fill_on_insert(&mut self, context: &FillContext) -> AppResult<()> {
        let now = from_chrono(context.now).db_context("参数配置自动填充时间")?;
        self.created_at = now;
        self.updated_at = now;
        self.id = crate::next_id()?;
        Ok(())
    }

    fn fill_on_update(&mut self, context: &FillContext) -> AppResult<()> {
        self.updated_at = from_chrono(context.now).db_context("参数配置自动填充时间")?;
        Ok(())
    }
}

impl ActiveModelBehavior for ActiveModel {}
