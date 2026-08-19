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
//! ```text
//! use chrono::{DateTime, Utc};
//! use ryframe_adapters::auto_fill::{AutoFill as AutoFillModel, FillContext};
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
//! // 使用 Snowflake 自动填充前，必须在进程启动边界完成一次初始化。
//! ryframe_adapters::snowflake::initialize(1).expect("初始化 Snowflake 失败");
//!
//! let mut user = User {
//!     id: 0,
//!     created_at: Utc::now(),
//!     login_date: None,
//! };
//! AutoFillModel::fill_on_insert(&mut user, &FillContext::new()).expect("自动填充失败");
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
/// ```text
/// use chrono::{DateTime, Utc};
/// use ryframe_adapters::auto_fill::{AutoFill as AutoFillModel, FillContext};
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
/// // 使用 Snowflake 自动填充前，必须在进程启动边界完成一次初始化。
/// ryframe_adapters::snowflake::initialize(1).expect("初始化 Snowflake 失败");
///
/// let mut user = User {
///     id: 0,
///     created_at: Utc::now(),
///     login_date: None,
/// };
/// AutoFillModel::fill_on_insert(&mut user, &FillContext::new()).expect("自动填充失败");
/// ```
#[proc_macro_derive(AutoFill, attributes(auto_fill))]
pub fn derive_auto_fill(input: TokenStream) -> TokenStream {
    auto_fill::expand_auto_fill(input)
}

/// 声明 GET 路由。将 `#[perm("code")]` 紧接放在该属性下方，以为生成的路由绑定权限。
#[proc_macro_attribute]
pub fn get(args: TokenStream, input: TokenStream) -> TokenStream {
    route::expand_route(route::HttpMethod::Get, args, input)
}

/// 声明 POST 路由。
#[proc_macro_attribute]
pub fn post(args: TokenStream, input: TokenStream) -> TokenStream {
    route::expand_route(route::HttpMethod::Post, args, input)
}

/// 声明 PUT 路由。
#[proc_macro_attribute]
pub fn put(args: TokenStream, input: TokenStream) -> TokenStream {
    route::expand_route(route::HttpMethod::Put, args, input)
}

/// 声明 PATCH 路由。
#[proc_macro_attribute]
pub fn patch(args: TokenStream, input: TokenStream) -> TokenStream {
    route::expand_route(route::HttpMethod::Patch, args, input)
}

/// 声明 DELETE 路由。
#[proc_macro_attribute]
pub fn delete(args: TokenStream, input: TokenStream) -> TokenStream {
    route::expand_route(route::HttpMethod::Delete, args, input)
}

/// 由上方路由属性消费的权限标记。
///
/// 若 `#[perm]` 进入展开阶段，说明属性顺序错误；否则会生成未受保护的路由。
#[proc_macro_attribute]
pub fn perm(_args: TokenStream, _input: TokenStream) -> TokenStream {
    "compile_error!(\"#[perm] 必须紧接在 #[get]、#[post]、#[put]、#[patch] 或 #[delete] 下方\");"
        .parse()
        .expect("valid compile_error output")
}

/// 由上方路由属性消费的产品能力标记。
///
/// 该标记同时被 API build script 扫描，用于生成权限码→Capability
/// 的编译期路由契约。
#[proc_macro_attribute]
pub fn capability(_args: TokenStream, _input: TokenStream) -> TokenStream {
    "compile_error!(\"#[capability] 必须紧接在 #[get]、#[post]、#[put]、#[patch] 或 #[delete] 下方\");"
        .parse()
        .expect("valid compile_error output")
}

/// 为路由处理函数构建生成的路由器，并将生成的辅助函数名隔离在应用代码之外。
///
/// ```text
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
