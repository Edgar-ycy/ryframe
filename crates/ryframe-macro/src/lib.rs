//! **ryframe-macro** — 过程宏 crate
//!
//! 提供 derive 宏：
//!
//! | 宏 | 种类 | 用途 |
//! |-----|------|------|
//! | `#[derive(AutoFill)]` | derive 宏 | 按默认规则自动填充实体字段（created_at 等），支持雪花 ID |
//!
//! # 用法
//!
//! ```ignore
//! use ryframe_macro::AutoFill;
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

use proc_macro::TokenStream;

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
