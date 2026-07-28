use std::ops::{Deref, DerefMut};

use async_trait::async_trait;
use ryframe_config::PaginationConfig;
use ryframe_kernel::{AppError, AppResult};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

/// 分页查询参数
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    /// 页码，从 1 开始
    pub page: u64,
    /// 每页记录数
    pub page_size: u64,
}

impl PageQuery {
    /// 根据可选查询参数和运行时生效策略解析 HTTP 请求分页值。HTTP 处理器必须使用此方法，
    /// 而非 `Default`，以确保 TOML 配置仍是省略参数的唯一依据。
    pub fn from_optional(
        page: Option<u64>,
        page_size: Option<u64>,
        policy: &PaginationConfig,
    ) -> AppResult<Self> {
        let query = Self {
            page: page.unwrap_or(1),
            page_size: page_size.unwrap_or(policy.default_page_size),
        };
        query.validate(policy)?;
        Ok(query)
    }

    /// 返回刻意提供非分页集合（如查找端点或导出批次）时使用的显式有界分页值。它与正常
    /// 请求分页保持分离。
    pub fn bounded_unpaged(policy: &PaginationConfig) -> AppResult<Self> {
        policy.validate().map_err(AppError::Config)?;
        Ok(Self {
            page: 1,
            page_size: policy.unpaged_max_records,
        })
    }

    /// 拒绝非法分页参数，避免静默裁剪客户端输入。
    pub fn validate(&self, policy: &PaginationConfig) -> AppResult<()> {
        if self.page == 0 {
            return Err(AppError::Validation("page must be greater than 0".into()));
        }
        if self.page_size == 0 {
            return Err(AppError::Validation(
                "page_size must be greater than 0".into(),
            ));
        }
        if self.page_size > policy.max_page_size {
            return Err(AppError::Validation(format!(
                "page_size must not exceed the configured maximum of {}",
                policy.max_page_size
            )));
        }
        if self
            .page
            .saturating_sub(1)
            .checked_mul(self.page_size)
            .is_none()
        {
            return Err(AppError::Validation(
                "page and page_size produce an offset that is too large".into(),
            ));
        }
        Ok(())
    }

    /// 计算 SQL 偏移量
    pub fn offset(&self) -> u64 {
        self.page.saturating_sub(1).saturating_mul(self.page_size)
    }
}

impl Default for PageQuery {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 10,
        }
    }
}

#[cfg(test)]
mod page_query_tests {
    use super::PageQuery;
    use ryframe_config::PaginationConfig;

    #[test]
    fn omitted_pagination_uses_runtime_policy_defaults() {
        let policy = PaginationConfig {
            default_page_size: 25,
            max_page_size: 100,
            unpaged_max_records: 1_000,
        };
        let query = PageQuery::from_optional(None, None, &policy).unwrap();

        assert_eq!(query.page, 1);
        assert_eq!(query.page_size, 25);
    }

    #[test]
    fn legacy_camel_case_pagination_is_rejected() {
        let error = serde_json::from_str::<PageQuery>(r#"{"pageSize": 20}"#)
            .expect_err("legacy pagination must not be accepted");

        assert!(error.to_string().contains("unknown field `pageSize`"));
    }

    #[test]
    fn invalid_pagination_is_rejected_without_clamping() {
        let policy = PaginationConfig::default();

        assert!(PageQuery::from_optional(Some(0), Some(10), &policy).is_err());
        assert!(PageQuery::from_optional(Some(1), Some(0), &policy).is_err());
        assert!(PageQuery::from_optional(Some(1), Some(101), &policy).is_err());
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
    pub fn new(records: Vec<T>, total: u64, query: &PageQuery) -> Self {
        Self {
            records,
            total,
            page: query.page,
            page_size: query.page_size,
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
        query: PageQuery,
    ) -> AppResult<PageResult<T>>;

    /// 插入新记录，返回插入后的实体
    async fn insert(&self, db: &DatabaseConnection, tenant_id: &str, entity: T) -> AppResult<T>;

    /// 更新记录，返回更新后的实体
    async fn update(&self, db: &DatabaseConnection, tenant_id: &str, entity: T) -> AppResult<T>;

    /// 根据主键删除记录
    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: ID) -> AppResult<()>;
}

/// 为保持 API 兼容性而保留的 Repository 包装器。它刻意绝不记录实体值：模型可能包含密码
/// 哈希、配置机密和其他凭据。
#[derive(Debug, Clone, Copy)]
pub struct LoggedRepo<R>(pub R);

impl<R> LoggedRepo<R> {
    /// 创建带日志的 Repository 包装器
    pub fn new(inner: R) -> Self {
        Self(inner)
    }
}

impl<R> Deref for LoggedRepo<R> {
    type Target = R;
    fn deref(&self) -> &R {
        &self.0
    }
}

impl<R> DerefMut for LoggedRepo<R> {
    fn deref_mut(&mut self) -> &mut R {
        &mut self.0
    }
}

#[async_trait]
impl<R, T, ID> Repository<T, ID> for LoggedRepo<R>
where
    R: Repository<T, ID> + Send + Sync,
    T: Send + Sync + 'static,
    ID: Send + Sync + 'static,
{
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: ID,
    ) -> AppResult<Option<T>> {
        self.0.find_by_id(db, tenant_id, id).await
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: PageQuery,
    ) -> AppResult<PageResult<T>> {
        self.0.find_by_page(db, tenant_id, query).await
    }

    async fn insert(&self, db: &DatabaseConnection, tenant_id: &str, entity: T) -> AppResult<T> {
        self.0.insert(db, tenant_id, entity).await
    }

    async fn update(&self, db: &DatabaseConnection, tenant_id: &str, entity: T) -> AppResult<T> {
        self.0.update(db, tenant_id, entity).await
    }

    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: ID) -> AppResult<()> {
        self.0.delete(db, tenant_id, id).await
    }
}
