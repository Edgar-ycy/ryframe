use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_login_info")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    /// 用户名
    pub user_name: String,
    /// 登录 IP
    pub ipaddr: String,
    /// 登录地点
    pub login_location: Option<String>,
    /// 浏览器
    pub browser: Option<String>,
    /// 操作系统
    pub os: Option<String>,
    /// 登录状态：0失败 1成功
    pub status: String,
    /// 提示信息
    pub msg: Option<String>,
    /// 登录时间
    pub login_time: DateTime<Utc>,
}

impl Model {
    pub const STATUS_SUCCESS: &str = "1";
    pub const STATUS_FAIL: &str = "0";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}