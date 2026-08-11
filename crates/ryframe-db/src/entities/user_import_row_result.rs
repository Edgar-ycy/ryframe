use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 导入中被跳过或验证失败的行。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_user_import_row_result")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub import_job_id: i64,
    pub row_number: i32,
    pub username_snapshot: String,
    pub outcome: String,
    pub code: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

impl Model {
    pub const OUTCOME_SKIPPED: &str = "skipped";
    pub const OUTCOME_FAILED: &str = "failed";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user_import_job::Entity",
        from = "Column::ImportJobId",
        to = "super::user_import_job::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    ImportJob,
}

impl Related<super::user_import_job::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ImportJob.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
