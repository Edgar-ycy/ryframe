use serde::Deserialize;
use utoipa::ToSchema;

/// 新建任务 DTO
#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct CreateJobDto {
    #[validate(length(min = 1, max = 100, message = "任务名称长度1-100"))]
    pub name: String,
    #[validate(length(min = 1, max = 100, message = "Cron 表达式长度1-100"))]
    pub cron_expr: String,
    pub group_name: Option<String>,
    pub misfire_policy: Option<String>,
    pub concurrent: Option<String>,
    pub remark: Option<String>,
}

/// 更新任务 DTO
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateJobDto {
    pub cron_expr: Option<String>,
    pub status: Option<String>,
    pub remark: Option<String>,
}
