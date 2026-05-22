use serde::Deserialize;

fn default_page() -> u64 { 1 }
fn default_page_size() -> u64 { 10 }

#[derive(Debug, Deserialize)]
pub struct OperLogPageQuery {
    #[serde(default = "default_page")]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
    pub oper_name: Option<String>,
    pub status: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
}