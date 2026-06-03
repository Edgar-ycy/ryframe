use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct CreateMenuDto {
    #[validate(length(min = 1, message = "菜单名称不能为空"))]
    pub name: String,
    pub parent_id: Option<i64>,
    /// 菜单类型: M目录 C菜单 F按钮
    #[validate(length(min = 1, max = 1, message = "菜单类型不能为空"))]
    pub menu_type: String,
    pub path: Option<String>,
    pub component: Option<String>,
    pub query: Option<String>,
    /// 权限标识(如 system:user:list)
    pub perms: Option<String>,
    pub icon: Option<String>,
    pub is_frame: Option<bool>,
    pub is_cache: Option<bool>,
    pub sort: Option<i32>,
    pub visible: Option<bool>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
pub struct UpdateMenuDto {
    #[validate(length(min = 1, message = "菜单名称不能为空"))]
    pub name: String,
    pub parent_id: Option<i64>,
    pub menu_type: String,
    pub path: Option<String>,
    pub component: Option<String>,
    pub query: Option<String>,
    pub perms: Option<String>,
    pub icon: Option<String>,
    pub is_frame: Option<bool>,
    pub is_cache: Option<bool>,
    pub sort: Option<i32>,
    pub visible: Option<bool>,
    pub status: String,
}
