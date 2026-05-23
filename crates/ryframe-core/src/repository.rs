use async_trait::async_trait;
use ryframe_common::AppResult;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};

/// 分页查询参数
#[derive(Debug, Clone, Deserialize)]
pub struct PageQuery {
    /// 页码，从 1 开始
    #[serde(default = "default_page")]
    pub page: u64,
    /// 每页记录数
    #[serde(default = "default_page_size", alias = "pageSize")]
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

    /// 转换为若依风格分页 API 响应
    pub fn to_page_response(self, msg: impl Into<String>) -> ryframe_common::ApiPageResponse<T>
    where
        T: Serialize,
    {
        ryframe_common::ApiPageResponse::new(self.records, self.total, msg)
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

/// 带结果日志的 Repository 包装器
///
/// 当 `sql_log_level = "full"` 时，自动在每次数据库操作后
/// 使用 `tracing::debug!` / 若依风格 `[结果]` 输出返回数据。
///
/// 通过 `Deref<Target = R>` 透明访问内部 Repository 的自定义方法。
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

/// 内部：以若依风格输出结果日志
fn log_full_result(label: &str, data: &dyn std::fmt::Debug) {
    if ryframe_common::is_sql_full_log() {
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), "[结果] {}: {:#?}", label, data);
    }
}

#[async_trait]
impl<R, T, ID> Repository<T, ID> for LoggedRepo<R>
where
    R: Repository<T, ID> + Send + Sync,
    T: std::fmt::Debug + Send + Sync + 'static,
    ID: std::fmt::Debug + Send + Sync + 'static,
{
    async fn find_by_id(&self, db: &DatabaseConnection, id: ID) -> AppResult<Option<T>> {
        let result = self.0.find_by_id(db, id).await;
        if let Ok(Some(ref data)) = result {
            log_full_result("find_by_id", data);
        }
        result
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<T>> {
        let result = self.0.find_by_page(db, query).await;
        if let Ok(ref page) = result {
            log_full_result(&format!("find_by_page (共{}条)", page.total), &page.records);
        }
        result
    }

    async fn insert(&self, db: &DatabaseConnection, entity: T) -> AppResult<T> {
        let result = self.0.insert(db, entity).await;
        if let Ok(ref data) = result {
            log_full_result("insert", data);
        }
        result
    }

    async fn update(&self, db: &DatabaseConnection, entity: T) -> AppResult<T> {
        let result = self.0.update(db, entity).await;
        if let Ok(ref data) = result {
            log_full_result("update", data);
        }
        result
    }

    async fn delete(&self, db: &DatabaseConnection, id: ID) -> AppResult<()> {
        let result = self.0.delete(db, id).await;
        if result.is_ok() {
            log_full_result("delete", &"success");
        }
        result
    }
}
