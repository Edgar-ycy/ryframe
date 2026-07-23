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
//! ```
//! use chrono::{DateTime, Utc};
//! use ryframe_core::auto_fill::{AutoFill as AutoFillModel, FillContext};
//! use ryframe_macro::AutoFill;
//!
//! // 自动填充（字段级标注，推荐）
//! #[derive(AutoFill)]
//! pub struct User {
//!     #[auto_fill(snowflake)]
//!     pub id: i64,
//!     pub created_at: DateTime<Utc>,
//!     #[auto_fill(skip)]
//!     pub login_date: Option<DateTime<Utc>>,
//! }
//!
//! let mut user = User {
//!     id: 0,
//!     created_at: Utc::now(),
//!     login_date: None,
//! };
//! AutoFillModel::fill_on_insert(&mut user, &FillContext::new()).expect("自动填充失败");
//! assert_ne!(user.id, 0);
//! ```

mod auto_fill;
mod route;

use proc_macro::TokenStream;
use quote::format_ident;
use syn::parse_macro_input;

/// 自动填充 derive 宏
///
/// 按 `DEFAULTS` 规则表自动填充实体字段（如 `created_at` → `Utc::now()`）。
/// 实体有对应字段则填充，没有则跳过。
///
/// 仅支持字段级标注：
/// - `#[auto_fill(snowflake)]`：插入时自动生成雪花 ID（用于主键 `id` 字段）
/// - `#[auto_fill(skip)]`：跳过默认规则，不自动填充
///
/// # 示例
///
/// ```
/// use chrono::{DateTime, Utc};
/// use ryframe_core::auto_fill::{AutoFill as AutoFillModel, FillContext};
/// use ryframe_macro::AutoFill;
///
/// #[derive(AutoFill)]
/// pub struct User {
///     #[auto_fill(snowflake)]
///     pub id: i64,
///     pub created_at: DateTime<Utc>,
///     #[auto_fill(skip)]
///     pub login_date: Option<DateTime<Utc>>,
/// }
///
/// let mut user = User {
///     id: 0,
///     created_at: Utc::now(),
///     login_date: None,
/// };
/// AutoFillModel::fill_on_insert(&mut user, &FillContext::new()).expect("自动填充失败");
/// assert_ne!(user.id, 0);
/// ```
#[proc_macro_derive(AutoFill, attributes(auto_fill))]
pub fn derive_auto_fill(input: TokenStream) -> TokenStream {
    auto_fill::expand_auto_fill(input)
}

/// Declare a GET route. Place `#[perm("code")]` immediately below this
/// attribute to bind the generated route to a permission.
#[proc_macro_attribute]
pub fn get(args: TokenStream, input: TokenStream) -> TokenStream {
    route::expand_route(route::HttpMethod::Get, args, input)
}

/// Declare a POST route.
#[proc_macro_attribute]
pub fn post(args: TokenStream, input: TokenStream) -> TokenStream {
    route::expand_route(route::HttpMethod::Post, args, input)
}

/// Declare a PUT route.
#[proc_macro_attribute]
pub fn put(args: TokenStream, input: TokenStream) -> TokenStream {
    route::expand_route(route::HttpMethod::Put, args, input)
}

/// Declare a DELETE route.
#[proc_macro_attribute]
pub fn delete(args: TokenStream, input: TokenStream) -> TokenStream {
    route::expand_route(route::HttpMethod::Delete, args, input)
}

/// Permission marker consumed by the route attribute above it.
///
/// `#[perm]` reaching expansion means the attributes were placed in the
/// wrong order, which would otherwise create an unprotected route.
#[proc_macro_attribute]
pub fn perm(_args: TokenStream, _input: TokenStream) -> TokenStream {
    "compile_error!(\"#[perm] must be placed immediately below #[get], #[post], #[put], or #[delete]\");"
        .parse()
        .expect("valid compile_error output")
}

/// Build the router generated for a route handler while keeping the generated
/// helper name out of application code.
///
/// ```
/// use axum::{Router, extract::State};
/// use ryframe_macro::{get, route};
///
/// #[derive(Clone)]
/// struct AppState;
///
/// #[get("/items")]
/// async fn list(State(_state): State<AppState>) {}
///
/// let _router: Router<AppState> = Router::new().merge(route!(list));
/// ```
#[proc_macro]
pub fn route(input: TokenStream) -> TokenStream {
    let mut handler = parse_macro_input!(input as syn::Path);
    let Some(segment) = handler.segments.last_mut() else {
        return "compile_error!(\"route! expects a handler function path\");"
            .parse()
            .expect("valid compile_error output");
    };

    segment.ident = format_ident!("__route_{}", segment.ident);
    quote::quote!(#handler()).into()
}
