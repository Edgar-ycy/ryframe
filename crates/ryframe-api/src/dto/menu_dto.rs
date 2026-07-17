use serde::Deserialize;
use utoipa::ToSchema;

use ryframe_service::system::MenuType;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateMenuDto {
    #[validate(length(min = 1, message = "菜单名称不能为空"))]
    pub name: String,
    /// Parent menu Snowflake ID, transported as a string.
    pub parent_id: Option<String>,
    /// Menu type: M directory, C page, F action.
    pub menu_type: MenuType,
    /// Permission ID. Buttons require it; directories/pages may also bind one.
    pub perm_id: Option<String>,
    /// Stable key used by the frontend page registry.
    #[validate(length(max = 100, message = "页面标识长度不能超过100"))]
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: Option<i32>,
    pub visible: Option<bool>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateMenuDto {
    #[validate(length(min = 1, message = "菜单名称不能为空"))]
    pub name: String,
    /// Parent menu Snowflake ID, transported as a string.
    pub parent_id: Option<String>,
    /// Menu type: M directory, C page, F action.
    pub menu_type: MenuType,
    pub perm_id: Option<String>,
    #[validate(length(max = 100, message = "页面标识长度不能超过100"))]
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: Option<i32>,
    pub visible: Option<bool>,
    pub status: String,
}
