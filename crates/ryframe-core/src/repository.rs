use async_trait::async_trait;
use ryframe_config::PaginationConfig;
use ryframe_kernel::{AppError, AppResult};
use sea_orm::DatabaseConnection;
use serde::Serialize;

/// 经过运行时策略校验的分页查询参数。
///
/// 字段保持私有，避免服务层和仓储层绕过页码、页大小及偏移量溢出校验。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPageQuery {
    page: u64,
    page_size: u64,
    offset: u64,
}

impl ValidatedPageQuery {
    /// 根据 API 层的可选原始参数和运行时策略创建分页值对象。
    pub fn from_optional(
        page: Option<u64>,
        page_size: Option<u64>,
        policy: &PaginationConfig,
    ) -> AppResult<Self> {
        Self::new(
            page.unwrap_or(1),
            page_size.unwrap_or(policy.default_page_size),
            policy,
        )
    }

    /// 使用运行时分页策略创建已校验的分页值对象。
    pub fn new(page: u64, page_size: u64, policy: &PaginationConfig) -> AppResult<Self> {
        policy.validate().map_err(AppError::Config)?;
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

#[cfg(test)]
mod page_query_tests {
    use super::{AppError, ValidatedPageQuery};
    use ryframe_config::PaginationConfig;

    #[test]
    fn omitted_pagination_uses_runtime_policy_defaults() {
        let policy = PaginationConfig {
            default_page_size: 25,
            max_page_size: 100,
        };
        let query = ValidatedPageQuery::from_optional(None, None, &policy).unwrap();

        assert_eq!(query.page(), 1);
        assert_eq!(query.page_size(), 25);
    }

    #[test]
    fn invalid_pagination_is_rejected_without_clamping() {
        let policy = PaginationConfig::default();

        assert!(ValidatedPageQuery::new(1, 1, &policy).is_ok());
        assert!(ValidatedPageQuery::new(1, 100, &policy).is_ok());
        assert!(ValidatedPageQuery::from_optional(Some(0), Some(10), &policy).is_err());
        assert!(ValidatedPageQuery::from_optional(Some(1), Some(0), &policy).is_err());
        assert!(ValidatedPageQuery::from_optional(Some(1), Some(101), &policy).is_err());
        assert!(ValidatedPageQuery::new(u64::MAX, 2, &policy).is_err());
    }

    #[test]
    fn invalid_runtime_policy_is_rejected() {
        let invalid_policies = [
            PaginationConfig {
                default_page_size: 0,
                max_page_size: 100,
            },
            PaginationConfig {
                default_page_size: 10,
                max_page_size: 0,
            },
            PaginationConfig {
                default_page_size: 101,
                max_page_size: 100,
            },
        ];

        for policy in invalid_policies {
            let error = ValidatedPageQuery::from_optional(None, None, &policy).unwrap_err();
            assert!(matches!(error, AppError::Config(_)));
        }
    }
}

/// 分页查询结果
#[derive(Debug, Clone, Serialize)]
pub struct PageResult<T> {
    /// 当前页数据
    pub records: Vec<T>,
    /// 总记录数
    pub total: u64,
    /// 当前页码
    pub page: u64,
    /// 每页记录数
    pub page_size: u64,
}

impl<T> PageResult<T> {
    /// 构造分页结果
    pub fn new(records: Vec<T>, total: u64, query: &ValidatedPageQuery) -> Self {
        Self {
            records,
            total,
            page: query.page(),
            page_size: query.page_size(),
        }
    }

    /// 总页数
    pub fn total_pages(&self) -> u64 {
        if self.page_size == 0 {
            return 0;
        }
        self.total.div_ceil(self.page_size)
    }
}

/// 通用 Repository 特征
///
/// `T` 为实体 Model 类型，`ID` 为主键类型。
#[async_trait]
pub trait Repository<T, ID>: Send + Sync {
    /// 根据主键查询单条记录
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: ID,
    ) -> AppResult<Option<T>>;

    /// 分页查询
    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<T>>;

    /// 插入新记录，返回插入后的实体
    async fn insert(&self, db: &DatabaseConnection, tenant_id: &str, entity: T) -> AppResult<T>;

    /// 更新记录，返回更新后的实体
    async fn update(&self, db: &DatabaseConnection, tenant_id: &str, entity: T) -> AppResult<T>;

    /// 根据主键删除记录
    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: ID) -> AppResult<()>;
}
