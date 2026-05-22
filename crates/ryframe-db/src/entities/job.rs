use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_job")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    /// 任务名（与 ScheduledTask::name() 对应）
    pub name: String,
    /// 任务分组（system/user/...）
    pub group_name: String,
    /// Cron 表达式（可覆盖代码默认值）
    pub cron_expr: String,
    /// 失败策略：1 立即执行 2 执行一次 3 放弃执行
    pub misfire_policy: String,
    /// 是否并发执行：0 禁止 1 允许
    pub concurrent: String,
    /// 状态：0 暂停 1 正常
    pub status: String,
    /// 备注
    pub remark: Option<String>,
    pub create_time: DateTime<Utc>,
    pub update_time: DateTime<Utc>,
}

impl Model {
    pub const STATUS_PAUSED: &str = "0";
    pub const STATUS_NORMAL: &str = "1";

    pub fn is_enabled(&self) -> bool {
        self.status == Self::STATUS_NORMAL
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
