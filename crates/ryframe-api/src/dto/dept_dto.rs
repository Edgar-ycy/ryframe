use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDeptDto {
    #[validate(length(min = 1, message = "部门名称不能为空"))]
    pub name: String,
    /// 父部门 Snowflake ID，统一使用字符串传输。
    pub parent_id: Option<String>,
    pub sort: Option<i32>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateDeptDto {
    #[validate(length(min = 1, message = "部门名称不能为空"))]
    pub name: String,
    /// 父部门 Snowflake ID，统一使用字符串传输。
    pub parent_id: Option<String>,
    pub sort: Option<i32>,
    pub status: String,
}
