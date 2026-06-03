//! **ryframe-macro** — 过程宏 crate
//!
//! 提供两类宏：
//!
//! | 宏 | 种类 | 用途 |
//! |-----|------|------|
//! | `#[datasource("name")]` | 属性宏 | 多数据源路由，标注在 async fn 上自动包裹 scope |
//! | `#[derive(AutoFill)]` | derive 宏 | 按默认规则自动填充实体字段（created_at 等） |
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
//! // 自动填充
//! #[derive(AutoFill)]
//! #[auto_fill(login_date, skip)]
//! struct User { pub created_at: DateTime<Utc>, pub login_date: Option<DateTime<Utc>> }
//! ```

mod auto_fill;
mod datasource;

use proc_macro::TokenStream;

/// 多数据源注解 — 类似 MyBatis-Plus `@DS("db_name")`
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
/// 用 `#[auto_fill(field_name, skip)]` 排除不想自动填充的字段。
///
/// # 示例
///
/// ```ignore
/// use ryframe_macro::AutoFill;
///
/// #[derive(AutoFill)]
/// #[auto_fill(login_date, skip)]
/// pub struct User {
///     pub created_at: DateTime<Utc>,
///     pub login_date: Option<DateTime<Utc>>,
/// }
/// ```
#[proc_macro_derive(AutoFill, attributes(auto_fill))]
pub fn derive_auto_fill(input: TokenStream) -> TokenStream {
    auto_fill::expand_auto_fill(input)
}
