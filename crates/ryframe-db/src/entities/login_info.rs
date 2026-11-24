use ryframe_macro::AutoFill;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::PrimitiveDateTime;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, AutoFill)]
#[sea_orm(table_name = "sys_login_info")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[auto_fill(snowflake)]
    pub id: i64,
    /// 认证成功或失败前所选的租户。
    pub tenant_id: String,
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
    #[serde(with = "super::oper_log::utc_time_serde")]
    pub login_time: PrimitiveDateTime,
}

impl Model {
    pub const STATUS_SUCCESS: &str = "1";
    pub const STATUS_FAIL: &str = "0";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
