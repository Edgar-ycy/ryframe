use serde::Deserialize;

#[derive(Debug, Deserialize, validator::Validate)]
pub struct CreateMenuDto {
    #[validate(length(min = 1, message = "菜单名称不能为空"))]
    pub name: String,
    pub parent_id: Option<i64>,
    pub path: Option<String>,
    pub component: Option<String>,
    pub icon: Option<String>,
    pub sort: Option<i32>,
    pub visible: Option<bool>,
}

#[derive(Debug, Deserialize, validator::Validate)]
pub struct UpdateMenuDto {
    #[validate(length(min = 1, message = "菜单名称不能为空"))]
    pub name: String,
    pub parent_id: Option<i64>,
    pub path: Option<String>,
    pub component: Option<String>,
    pub icon: Option<String>,
    pub sort: Option<i32>,
    pub visible: Option<bool>,
    pub status: String,
}
