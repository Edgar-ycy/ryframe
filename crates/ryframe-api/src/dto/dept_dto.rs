use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct CreateDeptDto {
    #[validate(length(min = 1, message = "部门名称不能为空"))]
    pub name: String,
    pub parent_id: Option<i64>,
    pub sort: Option<i32>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct UpdateDeptDto {
    #[validate(length(min = 1, message = "部门名称不能为空"))]
    pub name: String,
    pub parent_id: Option<i64>,
    pub sort: Option<i32>,
    pub status: String,
}
