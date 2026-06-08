use serde::Deserialize;
use utoipa::ToSchema;

fn default_page() -> u64 {
    1
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginLogPageQuery {
    #[serde(default = "default_page")]
    pub page: u64,
    #[serde(
        default = "ryframe_core::repository::default_page_size",
        alias = "pageSize"
    )]
    pub page_size: u64,
    pub user_name: Option<String>,
    pub status: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
}
