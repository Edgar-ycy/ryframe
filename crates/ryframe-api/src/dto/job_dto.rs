use serde::{Deserialize, Serialize};

/// 任务 VO
#[derive(Debug, Serialize)]
pub struct JobVO {
    pub id: i64,
    pub name: String,
    pub group_name: String,
    pub cron_expr: String,
    pub status: String,
    pub description: String,
    pub next_fire_time: Option<String>,
}

/// 更新任务 DTO
#[derive(Debug, Deserialize)]
pub struct UpdateJobDTO {
    pub cron_expr: Option<String>,
    pub status: Option<String>,
    pub remark: Option<String>,
}

/// 任务日志 VO
#[derive(Debug, Serialize)]
pub struct JobLogVO {
    pub id: i64,
    pub job_name: String,
    pub job_group: String,
    pub message: String,
    pub status: String,
    pub error_msg: Option<String>,
    pub cost_ms: i64,
}

/// 任务日志分页查询
#[derive(Debug, Deserialize)]
pub struct JobLogPageQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
    pub job_name: Option<String>,
    pub status: Option<String>,
}

fn default_page_size() -> u64 {
    10
}
