use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct CreatePermissionDto {
    #[validate(length(min = 1, max = 100, message = "权限名称长度1-100"))]
    pub name: String,
    #[validate(length(min = 1, max = 100, message = "权限码长度1-100"))]
    pub code: String,
    pub parent_id: Option<i64>,
    #[validate(length(min = 1, message = "权限类型不能为空"))]
    pub perm_type: String,
    pub icon: Option<String>,
    pub sort: Option<i32>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct UpdatePermissionDto {
    #[validate(length(min = 1, max = 100, message = "权限名称长度1-100"))]
    pub name: String,
    #[validate(length(min = 1, max = 100, message = "权限码长度1-100"))]
    pub code: String,
    pub parent_id: Option<i64>,
    #[validate(length(min = 1, message = "权限类型不能为空"))]
    pub perm_type: String,
    pub icon: Option<String>,
    pub sort: Option<i32>,
    pub status: Option<String>,
}
