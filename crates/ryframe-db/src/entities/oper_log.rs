use ryframe_macro::AutoFill;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::PrimitiveDateTime;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, AutoFill)]
#[sea_orm(table_name = "sys_oper_log")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[auto_fill(snowflake)]
    pub id: i64,
    /// 已认证请求上下文中绑定的租户。
    pub tenant_id: String,
    /// 用于保证至少一次投递幂等性的审计事件 UUID v7；旧数据允许为空。
    pub event_id: Option<String>,
    /// 与业务响应 `X-Request-Id` 一致的请求 UUID v7；旧数据允许为空。
    pub request_id: Option<String>,
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
    #[serde(with = "utc_time_serde")]
    pub oper_time: PrimitiveDateTime,
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

/// SQL 字段使用无偏移时间，实体的 JSON 时间沿用原 UTC 字符串。
pub(super) mod utc_time_serde {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::Error as _};
    use time::{OffsetDateTime, PrimitiveDateTime};

    pub fn serialize<S: Serializer>(
        value: &PrimitiveDateTime,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let value = value.assume_utc();
        DateTime::<Utc>::from_timestamp(value.unix_timestamp(), value.nanosecond())
            .ok_or_else(|| S::Error::custom("日志时间超出 UTC 表示范围"))?
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PrimitiveDateTime, D::Error> {
        let value = DateTime::<Utc>::deserialize(deserializer)?;
        let utc = OffsetDateTime::from_unix_timestamp(value.timestamp())
            .and_then(|utc| utc.replace_nanosecond(value.timestamp_subsec_nanos()))
            .map_err(de::Error::custom)?;
        Ok(PrimitiveDateTime::new(utc.date(), utc.time()))
    }
}
