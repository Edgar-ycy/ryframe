use serde::Deserialize;

fn default_page_size() -> u64 {
    10
}

fn default_max_page_size() -> u64 {
    100
}

/// HTTP 分页策略。
///
/// 请求处理器会在请求时从此策略解析缺省参数并创建已校验分页值；核心层不提供
/// 无策略的默认分页构造入口。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaginationConfig {
    /// API 调用方未提供 `page_size` 时使用的页大小。
    #[serde(default = "default_page_size")]
    pub default_page_size: u64,
    /// 常规分页端点接受的最大页大小。
    #[serde(default = "default_max_page_size")]
    pub max_page_size: u64,
}

impl PaginationConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.default_page_size == 0 {
            return Err("pagination.default_page_size must be greater than zero".into());
        }
        if self.max_page_size == 0 {
            return Err("pagination.max_page_size must be greater than zero".into());
        }
        if self.default_page_size > self.max_page_size {
            return Err(
                "pagination.default_page_size cannot exceed pagination.max_page_size".into(),
            );
        }
        Ok(())
    }
}

impl Default for PaginationConfig {
    fn default() -> Self {
        Self {
            default_page_size: default_page_size(),
            max_page_size: default_max_page_size(),
        }
    }
}
