use serde::Deserialize;
use utoipa::ToSchema;

use super::public_dto::MenuType;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateMenuDto {
    #[validate(length(min = 1, message = "菜单名称不能为空"))]
    pub name: String,
    /// 父菜单 Snowflake ID，以字符串传输。
    pub parent_id: Option<String>,
    /// 菜单类型：M 为目录，C 为页面，F 为操作。
    pub menu_type: MenuType,
    /// 权限 ID。按钮必须设置，目录和页面也可以绑定。
    pub perm_id: Option<String>,
    /// 前端页面注册表使用的稳定键。
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
    /// 父菜单 Snowflake ID，以字符串传输。
    pub parent_id: Option<String>,
    /// 菜单类型：M 为目录，C 为页面，F 为操作。
    pub menu_type: MenuType,
    pub perm_id: Option<String>,
    #[validate(length(max = 100, message = "页面标识长度不能超过100"))]
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: Option<i32>,
    pub visible: Option<bool>,
    pub status: String,
}
