use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
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
pub struct UpdateRoleDto {
    #[validate(length(min = 1, message = "角色名称不能为空"))]
    pub name: String,
    pub sort: Option<i32>,
    pub status: String,
    pub data_scope: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AssignPermsDto {
    /// 权限ID列表（接受 number|string，前端 Snowflake ID 以字符串传输）
    #[serde(default)]
    pub perm_ids: Vec<String>,
}

/// 数据权限分配请求
#[derive(Debug, Deserialize, ToSchema)]
pub struct AssignDataScopeDto {
    /// 数据范围: "1"全部 "2"自定义 "3"本部门 "4"本部门及以下 "5"仅本人
    pub data_scope: String,
    /// 自定义部门ID列表（仅 data_scope="2" 时有效，接受 number|string）
    #[serde(default)]
    pub dept_ids: Vec<String>,
}
