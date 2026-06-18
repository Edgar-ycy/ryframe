use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateMenuDto {
    #[validate(length(min = 1, message = "菜单名称不能为空"))]
    pub name: String,
    /// Parent menu ID. Snowflake IDs may be sent as strings by the frontend.
    pub parent_id: Option<String>,
    /// Menu type: M directory, C page, F action.
    #[validate(length(min = 1, max = 1, message = "菜单类型不能为空"))]
    pub menu_type: String,
    pub icon: Option<String>,
    pub sort: Option<i32>,
    pub visible: Option<bool>,
}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateMenuDto {
    #[validate(length(min = 1, message = "菜单名称不能为空"))]
    pub name: String,
    /// Parent menu ID. Snowflake IDs may be sent as strings by the frontend.
    pub parent_id: Option<String>,
    /// Menu type: M directory, C page, F action.
    #[validate(length(min = 1, max = 1, message = "菜单类型不能为空"))]
    pub menu_type: String,
    pub icon: Option<String>,
    pub sort: Option<i32>,
    pub visible: Option<bool>,
    pub status: String,
}
