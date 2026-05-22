use serde::Deserialize;

#[derive(Debug, Deserialize, validator::Validate)]
pub struct CreateConfigDto {
    #[validate(length(min = 1, max = 100, message = "参数名称长度为1-100"))]
    pub name: String,
    #[validate(length(min = 1, max = 100, message = "参数键名长度为1-100"))]
    pub key: String,
    #[validate(length(min = 1, max = 500, message = "参数键值长度为1-500"))]
    pub value: String,
    pub remark: Option<String>,
}

#[derive(Debug, Deserialize, validator::Validate)]
pub struct UpdateConfigDto {
    #[validate(length(min = 1, message = "配置值不能为空"))]
    pub value: String,
}
