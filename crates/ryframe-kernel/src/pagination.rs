use serde::Serialize;

use crate::{AppError, AppResult};

/// 与传输和配置加载无关的分页策略值对象。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaginationPolicy {
    default_page_size: u64,
    max_page_size: u64,
}

impl PaginationPolicy {
    /// 使用配置层给出的限制构造策略，实际请求解析时会统一校验。
    pub const fn new(default_page_size: u64, max_page_size: u64) -> Self {
        Self {
            default_page_size,
            max_page_size,
        }
    }

    const fn default_page_size(self) -> u64 {
        self.default_page_size
    }

    fn validate(self) -> AppResult<()> {
        if self.default_page_size == 0 {
            return Err(AppError::Config(
                "pagination.default_page_size must be greater than zero".into(),
            ));
        }
        if self.max_page_size == 0 {
            return Err(AppError::Config(
                "pagination.max_page_size must be greater than zero".into(),
            ));
        }
        if self.default_page_size > self.max_page_size {
            return Err(AppError::Config(
                "pagination.default_page_size cannot exceed pagination.max_page_size".into(),
            ));
        }
        Ok(())
    }
}

/// 经过运行时策略校验的分页查询参数。
///
/// 字段保持私有，避免调用方绕过页码、页大小及偏移量溢出校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedPageQuery {
    page: u64,
    page_size: u64,
    offset: u64,
}

impl ValidatedPageQuery {
    /// 根据可选原始参数和运行时策略创建分页值对象。
    pub fn from_optional<P>(page: Option<u64>, page_size: Option<u64>, policy: P) -> AppResult<Self>
    where
        P: Into<PaginationPolicy>,
    {
        let policy = policy.into();
        Self::new(
            page.unwrap_or(1),
            page_size.unwrap_or(policy.default_page_size()),
            policy,
        )
    }

    /// 使用运行时分页策略创建已校验的分页值对象。
    pub fn new<P>(page: u64, page_size: u64, policy: P) -> AppResult<Self>
    where
        P: Into<PaginationPolicy>,
    {
        let policy = policy.into();
        policy.validate()?;
        if page == 0 {
            return Err(AppError::Validation("page must be greater than 0".into()));
        }
        if page_size == 0 {
            return Err(AppError::Validation(
                "page_size must be greater than 0".into(),
            ));
        }
        if page_size > policy.max_page_size {
            return Err(AppError::Validation(format!(
                "page_size must not exceed the configured maximum of {}",
                policy.max_page_size
            )));
        }
        let offset = page
            .checked_sub(1)
            .and_then(|page_index| page_index.checked_mul(page_size))
            .ok_or_else(|| {
                AppError::Validation(
                    "page and page_size produce an offset that is too large".into(),
                )
            })?;
        Ok(Self {
            page,
            page_size,
            offset,
        })
    }

    /// 返回从 1 开始的页码。
    pub const fn page(&self) -> u64 {
        self.page
    }

    /// 返回每页记录数。
    pub const fn page_size(&self) -> u64 {
        self.page_size
    }

    /// 返回已校验且不会溢出的 SQL 偏移量。
    pub const fn offset(&self) -> u64 {
        self.offset
    }
}

/// 分页查询结果。
#[derive(Debug, Clone, Serialize)]
pub struct PageResult<T> {
    /// 当前页数据。
    pub records: Vec<T>,
    /// 总记录数。
    pub total: u64,
    /// 当前页码。
    pub page: u64,
    /// 每页记录数。
    pub page_size: u64,
}

impl<T> PageResult<T> {
    /// 构造分页结果。
    pub fn new(records: Vec<T>, total: u64, query: &ValidatedPageQuery) -> Self {
        Self {
            records,
            total,
            page: query.page(),
            page_size: query.page_size(),
        }
    }

    /// 返回总页数。
    pub fn total_pages(&self) -> u64 {
        if self.page_size == 0 {
            return 0;
        }
        self.total.div_ceil(self.page_size)
    }
}

#[cfg(test)]
mod tests {
    use super::{PageResult, PaginationPolicy, ValidatedPageQuery};
    use crate::AppError;

    const POLICY: PaginationPolicy = PaginationPolicy::new(10, 100);

    #[test]
    fn applies_defaults_and_calculates_offset() {
        let query = ValidatedPageQuery::from_optional(Some(3), None, POLICY).unwrap();

        assert_eq!(query.page(), 3);
        assert_eq!(query.page_size(), 10);
        assert_eq!(query.offset(), 20);
    }

    #[test]
    fn rejects_invalid_requests_and_overflow() {
        assert!(matches!(
            ValidatedPageQuery::new(0, 10, POLICY),
            Err(AppError::Validation(_))
        ));
        assert!(matches!(
            ValidatedPageQuery::new(1, 101, POLICY),
            Err(AppError::Validation(_))
        ));
        assert!(matches!(
            ValidatedPageQuery::new(u64::MAX, 2, POLICY),
            Err(AppError::Validation(_))
        ));
    }

    #[test]
    fn calculates_total_pages_without_copying_records() {
        let query = ValidatedPageQuery::new(1, 10, POLICY).unwrap();
        let result = PageResult::new(vec!["a", "b"], 21, &query);

        assert_eq!(result.total_pages(), 3);
        assert_eq!(result.records, vec!["a", "b"]);
    }
}
