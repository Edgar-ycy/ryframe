use async_trait::async_trait;
use ryframe_common::AppResult;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

/// 分页查询参数
#[derive(Debug, Clone, Deserialize)]
pub struct PageQuery {
    /// 页码，从 1 开始
    #[serde(default = "default_page")]
    pub page: u64,
    /// 每页记录数
    #[serde(default = "default_page_size")]
    pub page_size: u64,
}

fn default_page() -> u64 {
    1
}

fn default_page_size() -> u64 {
    10
}

impl PageQuery {
    /// 计算 SQL OFFSET 值
    pub fn offset(&self) -> u64 {
        (self.page.saturating_sub(1)) * self.page_size
    }

    /// 规范化分页参数（限制最大值，防止传入非法值）
    pub fn normalize(mut self, max_page_size: u64) -> Self {
        if self.page_size > max_page_size {
            self.page_size = max_page_size;
        }
        if self.page_size == 0 {
            self.page_size = 10;
        }
        if self.page == 0 {
            self.page = 1;
        }
        self
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

/// 通用 Repository trait
///
/// `T` 为实体 Model 类型，`ID` 为主键类型。
#[async_trait]
pub trait Repository<T, ID>: Send + Sync {
    /// 根据主键查询单条记录
    async fn find_by_id(&self, db: &DatabaseConnection, id: ID) -> AppResult<Option<T>>;

    /// 分页查询
    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<T>>;

    /// 插入新记录，返回插入后的实体
    async fn insert(&self, db: &DatabaseConnection, entity: T) -> AppResult<T>;

    /// 更新记录，返回更新后的实体
    async fn update(&self, db: &DatabaseConnection, entity: T) -> AppResult<T>;

    /// 根据主键删除记录
    async fn delete(&self, db: &DatabaseConnection, id: ID) -> AppResult<()>;
}
