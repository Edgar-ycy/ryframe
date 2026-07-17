use serde::Deserialize;
use utoipa::ToSchema;

use ryframe_service::system::PermissionType;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePermissionDto {
    #[validate(length(min = 1, max = 100, message = "权限名称长度1-100"))]
    pub name: String,
    #[validate(length(min = 1, max = 100, message = "权限码长度1-100"))]
    pub code: String,
    pub parent_id: Option<String>,
    pub perm_type: PermissionType,
    pub icon: Option<String>,
    pub sort: Option<i32>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePermissionDto {
    #[validate(length(min = 1, max = 100, message = "权限名称长度1-100"))]
    pub name: String,
    #[validate(length(min = 1, max = 100, message = "权限码长度1-100"))]
    pub code: String,
    pub parent_id: Option<String>,
    pub perm_type: PermissionType,
    pub icon: Option<String>,
    pub sort: Option<i32>,
    pub status: Option<String>,
}
