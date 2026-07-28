use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 消息发布时固化的受众选择器。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_message_audience")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub message_id: i64,
    pub tenant_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub kind: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub target_id: i64,
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
