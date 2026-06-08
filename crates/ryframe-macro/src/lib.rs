//! **ryframe-macro** — 过程宏 crate
//!
//! 提供两类宏：
//!
//! | 宏 | 种类 | 用途 |
//! |-----|------|------|
//! | `#[datasource("name")]` | 属性宏 | 多数据源路由，标注在 async fn 上自动包裹 scope |
//! | `#[derive(AutoFill)]` | derive 宏 | 按默认规则自动填充实体字段（created_at 等），支持雪花 ID |
//!
//! # 用法
//!
//! ```ignore
//! use ryframe_macro::{datasource, AutoFill};
//!
//! // 多数据源
//! #[datasource("db_device")]
//! async fn list_devices() { ... }
//!
//! // 自动填充（字段级标注，推荐）
//! #[derive(AutoFill)]
//! pub struct User {
//!     #[sea_orm(primary_key, auto_increment = false)]
//!     #[auto_fill(snowflake)]
//!     pub id: i64,
//!     pub created_at: DateTime<Utc>,
//!     #[auto_fill(skip)]
//!     pub login_date: Option<DateTime<Utc>>,
//! }
//!
//! // struct 级标注也支持（兼容旧写法）
//! #[derive(AutoFill)]
//! #[auto_fill(login_date, skip)]
//! struct User { pub created_at: DateTime<Utc>, pub login_date: Option<DateTime<Utc>> }
//! ```

mod auto_fill;
mod datasource;

use proc_macro::TokenStream;

/// 多数据源注解
///
/// 标注在 async 函数上，自动将函数体包裹在目标数据源上下文中。
/// 函数返回时自动恢复之前的数据源（支持嵌套）。
///
/// `task_local!.scope()` 自动处理嵌套和恢复。
///
/// # 标注位置
///
/// - **Handler 层**：直接标注在 axum handler 函数上
/// - **Service 层**：标注在 `impl Service` 的具体方法上
///
/// # 示例
///
/// ```ignore
/// use ryframe_macro::datasource;
///
/// #[datasource("db_device")]
/// pub async fn list_devices(&self, query: PageQuery) -> AppResult<PageResult<DeviceVo>> {
///     let db = self.device_repo.db(); // ← 自动解析为 db_device 连接
///     self.device_repo.find_by_page(&db, query).await
/// }
/// ```
#[proc_macro_attribute]
pub fn datasource(attr: TokenStream, item: TokenStream) -> TokenStream {
    datasource::expand_datasource(attr, item)
}

/// 自动填充 derive 宏
///
/// 按 `DEFAULTS` 规则表自动填充实体字段（如 `created_at` → `Utc::now()`）。
/// 实体有对应字段则填充，没有则跳过。
///
/// 支持字段级和 struct 级两种标注方式：
/// - `#[auto_fill(snowflake)]`：插入时自动生成雪花 ID（用于主键 `id` 字段）
/// - `#[auto_fill(skip)]`：跳过默认规则，不自动填充
/// - `#[auto_fill(field_name, skip)]`：struct 级跳过（兼容旧写法）
/// - `#[auto_fill(field_name, snowflake)]`：struct 级雪花 ID（兼容旧写法）
///
/// # 示例
///
/// ```ignore
/// use ryframe_macro::AutoFill;
///
/// #[derive(AutoFill)]
/// pub struct User {
///     #[sea_orm(primary_key, auto_increment = false)]
///     #[auto_fill(snowflake)]
///     pub id: i64,
///     pub created_at: DateTime<Utc>,
///     #[auto_fill(skip)]
///     pub login_date: Option<DateTime<Utc>>,
/// }
/// ```
#[proc_macro_derive(AutoFill, attributes(auto_fill))]
pub fn derive_auto_fill(input: TokenStream) -> TokenStream {
    auto_fill::expand_auto_fill(input)
}
