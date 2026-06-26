use chrono::{DateTime, Utc};
use ryframe_macro::AutoFill;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, AutoFill)]
#[sea_orm(table_name = "sys_oper_log")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[auto_fill(snowflake)]
    pub id: i64,
    /// Tenant bound in the authenticated request context.
    pub tenant_id: String,
    /// 模块标题
    pub title: String,
    /// 业务类型（INSERT/UPDATE/DELETE/EXPORT/IMPORT/GRANT 等）
    pub business_type: String,
    /// 操作方法（类名.方法名）
    pub method: String,
    /// 请求方式（GET/POST/PUT/DELETE）
    pub request_method: String,
    /// 操作人员
    pub oper_name: String,
    /// 请求 URL
    pub oper_url: String,
    /// 操作 IP
    pub oper_ip: String,
    /// 操作地点（根据 IP 解析，可空）
    pub oper_location: Option<String>,
    /// 请求参数（JSON 字符串）
    pub oper_param: Option<String>,
    /// 返回结果（JSON 字符串）
    pub json_result: Option<String>,
    /// 操作状态：0异常 1成功
    pub status: String,
    /// 错误信息
    pub error_msg: Option<String>,
    /// 操作时间
    pub oper_time: DateTime<Utc>,
    /// 耗时（毫秒）
    pub cost_time: i64,
}

impl Model {
    pub const STATUS_SUCCESS: &str = "1";
    pub const STATUS_FAIL: &str = "0";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
