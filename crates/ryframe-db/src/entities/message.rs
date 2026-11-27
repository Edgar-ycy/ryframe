use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// 消息中心的持久化消息。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_message")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub topic: String,
    pub title_text: Option<String>,
    pub body_text: Option<String>,
    pub title_key: Option<String>,
    pub body_key: Option<String>,
    pub args_json: Option<Json>,
    pub severity: String,
    pub payload_json: Option<Json>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub created_by: Option<i64>,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub published_at: OffsetDateTime,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub created_at: OffsetDateTime,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub updated_at: OffsetDateTime,
}

impl Model {
    pub const SEVERITY_INFO: &str = "info";
    pub const SEVERITY_SUCCESS: &str = "success";
    pub const SEVERITY_WARNING: &str = "warning";
    pub const SEVERITY_ERROR: &str = "error";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::message_audience::Entity")]
    Audience,
    #[sea_orm(has_many = "super::message_recipient::Entity")]
    Recipient,
}

impl Related<super::message_audience::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Audience.def()
    }
}

impl Related<super::message_recipient::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Recipient.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
