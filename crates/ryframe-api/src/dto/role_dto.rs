use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRoleDto {
    #[validate(length(min = 1, max = 50, message = "角色名称长度1-50"))]
    pub name: String,
    #[validate(length(min = 1, max = 50, message = "角色编码长度1-50"))]
    pub code: String,
    pub sort: Option<i32>,
    /// 数据范围: "1"全部 "2"自定义 "3"本部门 "4"本部门及以下 "5"仅本人
    pub data_scope: Option<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRoleDto {
    #[validate(length(min = 1, message = "角色名称不能为空"))]
    pub name: String,
    pub sort: Option<i32>,
    pub status: String,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct RolePermAssignDto {
    pub role_id: String,
    #[serde(default)]
    pub perm_ids: Vec<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct RoleDeptAssignDto {
    pub role_id: String,
    #[serde(default)]
    pub dept_ids: Vec<String>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RoleDataScopeUpdateDto {
    pub role_id: String,
    #[validate(custom(function = "validate_data_scope"))]
    pub data_scope: String,
}

fn validate_data_scope(value: &str) -> Result<(), validator::ValidationError> {
    if matches!(value, "1" | "2" | "3" | "4" | "5") {
        Ok(())
    } else {
        Err(validator::ValidationError::new("invalid_data_scope"))
    }
}
