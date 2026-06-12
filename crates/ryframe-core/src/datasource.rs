//! 多数据源连接注册中心
//!
//! `DataSourceManager`: 启动时注册所有数据源连接，运行时按名称解析。
//!
//! 数据源切换通过显式传参模式实现：
//! - Handler 层调用 `state.write_db()` / `state.read_db()` 选择连接
//! - 将 `&DatabaseConnection` 显式传入 Service → Repository 调用链

use std::sync::Arc;

use dashmap::DashMap;
use sea_orm::DatabaseConnection;

// ============================================================
// 多数据源连接注册中心
// ============================================================

/// 多数据源连接管理器
///
/// 启动时注册所有数据源（primary / replicas / 命名数据源），
/// 运行时按名称直接获取对应连接。
///
/// # 使用方式
///
/// ```
/// use ryframe_core::DataSourceManager;
///
/// // 创建并注册数据源（实际使用时需传入 DatabaseConnection）
/// let manager = DataSourceManager::new();
/// assert!(manager.is_empty());
/// assert_eq!(manager.len(), 0);
/// ```
#[derive(Clone)]
pub struct DataSourceManager {
    connections: Arc<DashMap<String, DatabaseConnection>>,
}

impl DataSourceManager {
    /// 创建空管理器
    pub fn new() -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
        }
    }

    /// 注册一个命名数据源连接
    ///
    /// # 参数
    /// - `name`: 数据源名称，如 `"primary"`, `"replica_0"`, `"db_device"`
    /// - `db`: SeaORM `DatabaseConnection` 连接池
    pub fn register(&self, name: impl Into<String>, db: DatabaseConnection) {
        let name = name.into();
        tracing::info!("[DataSourceManager] 注册数据源: {}", name);
        self.connections.insert(name, db);
    }

    /// 按名称获取连接，不存在返回 `None`
    pub fn get(&self, name: &str) -> Option<DatabaseConnection> {
        self.connections.get(name).map(|r| r.clone())
    }

    /// 获取 primary 连接（必须已注册，否则 panic）
    pub fn primary(&self) -> DatabaseConnection {
        self.get("primary")
            .expect("[DataSourceManager] primary 数据源必须注册")
    }

    /// 获取所有已注册的数据源名称列表
    pub fn names(&self) -> Vec<String> {
        self.connections.iter().map(|r| r.key().clone()).collect()
    }

    /// 已注册的数据源数量
    pub fn len(&self) -> usize {
        self.connections.len()
    }

    /// 是否无任何注册数据源
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }
}

impl Default for DataSourceManager {
    fn default() -> Self {
        Self::new()
    }
}
