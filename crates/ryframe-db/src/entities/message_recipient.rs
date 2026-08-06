use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 消息收件箱快照；同一消息和用户只保留一条记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_message_recipient")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub message_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: i64,
    pub tenant_id: String,
    pub created_at: DateTime<Utc>,
    pub enqueued_at: Option<DateTime<Utc>>,
    pub acked_at: Option<DateTime<Utc>>,
    pub read_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::message::Entity",
        from = "Column::MessageId",
        to = "super::message::Column::Id"
    )]
    Message,
}

impl Related<super::message::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Message.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
