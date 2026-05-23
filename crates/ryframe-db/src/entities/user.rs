use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_user")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    #[sea_orm(unique)]
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub status: String,
    pub dept_id: Option<i64>,
    pub remark: Option<String>,
    pub login_ip: Option<String>,
    pub login_date: Option<DateTime<Utc>>,
    pub del_flag: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 用户状态常量
impl Model {
    pub const STATUS_DISABLED: &str = "0";
    pub const STATUS_NORMAL: &str = "1";
    pub const STATUS_LOCKED: &str = "2";

    pub const DEL_FLAG_NORMAL: &str = "0";
    pub const DEL_FLAG_DELETED: &str = "2";

    pub fn is_enabled(&self) -> bool {
        self.status == Self::STATUS_NORMAL
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::user_role::Entity")]
    UserRole,
}

impl Related<super::user_role::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::UserRole.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_model() {
        assert_eq!(Model::STATUS_NORMAL, "1");
        assert_eq!(Model::STATUS_DISABLED, "0");
        assert_eq!(Model::STATUS_LOCKED, "2");

        let now = Utc::now();
        let mut m = Model {
            id: 1,
            username: "test".into(),
            password_hash: "x".into(),
            nickname: "t".into(),
            email: "".into(),
            phone: "".into(),
            avatar: None,
            status: Model::STATUS_NORMAL.to_string(),
            dept_id: None,
            remark: None,
            login_ip: None,
            login_date: None,
            del_flag: Model::DEL_FLAG_NORMAL.to_string(),
            created_at: now,
            updated_at: now,
        };
        assert!(m.is_enabled());
        m.status = Model::STATUS_DISABLED.to_string();
        assert!(!m.is_enabled());
    }
}
