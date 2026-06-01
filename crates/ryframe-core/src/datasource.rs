//! 多数据源上下文持有与连接注册中心
//!
//! - `DataSourceContext`: 基于 task_local 的数据源上下文持有，实现线程级数据源切换。
//!   基于 `tokio::task::task_local!` 的 task-local 存储
//! - `DataSourceManager`: 启动时注册所有数据源连接，运行时按名称解析
//! - 全局单例: 通过 `set_global()` / `global()` / `current_db()` 避免
//!   所有 Repository 都需要显式注入 `DataSourceManager`

use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use sea_orm::DatabaseConnection;

// ============================================================
// 全局单例（减少注入样板代码）
// ============================================================

static GLOBAL_MANAGER: OnceLock<Arc<DataSourceManager>> = OnceLock::new();

/// 获取当前 task-local 上下文对应的 DatabaseConnection
///
/// 此函数通过全局 `DataSourceManager` 单例解析，无需显式注入。
/// Service / Repository 层可直接调用。
pub fn current_db() -> DatabaseConnection {
    DataSourceManager::global().current_db()
}

/// 按名称获取全局注册的数据源连接
pub fn get_db(name: &str) -> Option<DatabaseConnection> {
    DataSourceManager::global().get(name)
}

// ============================================================
// Task-local 数据源名称
// ============================================================

tokio::task_local! {
    /// 当前 task 绑定的数据源名称
    ///
    /// 由 `#[datasource("name")]` proc-macro 通过 `.scope()` 设置，
    /// 也可手动调用 `.scope(name, fut).await`。
    /// 未设置时 `DataSourceContext::current_name()` 返回 `"primary"`。
    pub static DATA_SOURCE_NAME: String;
}

/// 数据源上下文工具
pub struct DataSourceContext;

impl DataSourceContext {
    /// 获取当前数据源名称，未设置时返回 `"primary"`
    pub fn current_name() -> String {
        DATA_SOURCE_NAME
            .try_with(|n| n.clone())
            .unwrap_or_else(|_| "primary".to_string())
    }

    /// 尝试获取当前数据源名称，未设置返回 `None`
    pub fn try_current_name() -> Option<String> {
        DATA_SOURCE_NAME.try_with(|n| n.clone()).ok()
    }
}

// ============================================================
// 多数据源连接注册中心
// ============================================================

/// 多数据源连接管理器
///
/// 启动时注册所有数据源（primary / replicas / 命名数据源），
/// 运行时通过 `current_db()` 从 `DATA_SOURCE_NAME` task-local 解析对应连接。
///
/// # 使用方式
///
/// ```
/// use ryframe_core::datasource::DataSourceManager;
///
/// // 创建并注册数据源（实际使用时需传入 DatabaseConnection）
/// let manager = DataSourceManager::new();
/// assert!(manager.is_empty());
/// assert_eq!(manager.len(), 0);
///
/// // 运行时通过全局单例解析
/// // manager.set_global();
/// // let db = manager.current_db();
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

    /// 设为全局单例（在 main.rs 启动阶段调用一次）
    ///
    /// 设置后可通过 `DataSourceManager::global()` 和 `current_db()` 访问。
    pub fn set_global(self) {
        GLOBAL_MANAGER
            .set(Arc::new(self))
            .map_err(|_| {
                tracing::warn!("[DataSourceManager] 全局实例已设置，忽略重复调用");
            })
            .ok();
    }

    /// 获取全局单例引用
    pub fn global() -> Arc<DataSourceManager> {
        GLOBAL_MANAGER
            .get()
            .cloned()
            .expect("[DataSourceManager] 全局实例未初始化，请在 main.rs 启动时调用 set_global()")
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

    /// 根据当前 task-local 上下文解析连接
    ///
    /// 查找顺序：
    /// 1. `DATA_SOURCE_NAME` task-local 中设置的值
    /// 2. 回退到 `"primary"`
    /// 3. 若指定名称未注册 → 记录 warning 并回退到 primary
    pub fn current_db(&self) -> DatabaseConnection {
        let ds_name = DataSourceContext::current_name();
        match self.get(&ds_name) {
            Some(db) => db,
            None => {
                if ds_name != "primary" {
                    tracing::warn!(
                        "[DataSourceManager] 数据源 '{}' 未注册，回退到 primary",
                        ds_name
                    );
                }
                self.primary()
            }
        }
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
