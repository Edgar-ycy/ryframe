use serde::Deserialize;

/// 对有意返回有限非分页集合的端点设置绝对保护上限，例如小型查询列表或导出批次。
/// 该值可在此上限以内配置，以防部署文件的笔误让单个请求变成无界查询。
pub const HARD_MAX_UNPAGED_RECORDS: u64 = 10_000;

fn default_page_size() -> u64 {
    10
}

fn default_max_page_size() -> u64 {
    100
}

fn default_unpaged_max_records() -> u64 {
    1_000
}

/// HTTP 分页策略。
///
/// 请求处理器会在请求时从此策略解析缺省的分页参数；核心层
/// `PageQuery::default()` 的值不会被当作 HTTP 契约。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaginationConfig {
    /// API 调用方未提供 `page_size` 时使用的页大小。
    #[serde(default = "default_page_size")]
    pub default_page_size: u64,
    /// 常规分页端点接受的最大页大小。
    #[serde(default = "default_max_page_size")]
    pub max_page_size: u64,
    /// 有意不分页的端点允许返回的最大记录数。
    #[serde(default = "default_unpaged_max_records")]
    pub unpaged_max_records: u64,
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
        if self.unpaged_max_records == 0 {
            return Err("pagination.unpaged_max_records must be greater than zero".into());
        }
        if self.unpaged_max_records > HARD_MAX_UNPAGED_RECORDS {
            return Err(format!(
                "pagination.unpaged_max_records cannot exceed {HARD_MAX_UNPAGED_RECORDS}"
            ));
        }
        Ok(())
    }
}

impl Default for PaginationConfig {
    fn default() -> Self {
        Self {
            default_page_size: default_page_size(),
            max_page_size: default_max_page_size(),
            unpaged_max_records: default_unpaged_max_records(),
        }
    }
}
