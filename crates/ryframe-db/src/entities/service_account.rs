use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 仅用于 Agent API 的租户服务账号，不能参与普通用户登录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_service_account")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub code: String,
    pub name: String,
    pub description: Option<String>,
    pub dept_id: Option<i64>,
    pub status: String,
    pub authorization_version: i32,
    pub max_requests_per_minute: i32,
    pub created_by: i64,
    pub del_flag: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const STATUS_DISABLED: &str = "0";
    pub const STATUS_NORMAL: &str = "1";
    pub const DEL_FLAG_NORMAL: &str = "0";
    pub const DEL_FLAG_DELETED: &str = "2";

    pub fn is_enabled(&self) -> bool {
        self.status == Self::STATUS_NORMAL && self.del_flag == Self::DEL_FLAG_NORMAL
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
