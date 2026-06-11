use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct CreateDeptDto {
    #[validate(length(min = 1, message = "部门名称不能为空"))]
    pub name: String,
    /// 父部门ID（接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失）
    pub parent_id: Option<String>,
    pub sort: Option<i32>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct UpdateDeptDto {
    #[validate(length(min = 1, message = "部门名称不能为空"))]
    pub name: String,
    /// 父部门ID（接受 number|string，前端 Snowflake ID 以字符串传输避免 JS 精度丢失）
    pub parent_id: Option<String>,
    pub sort: Option<i32>,
    pub status: String,
}
