//! 自动填充 derive 宏
//!
//! 按 `DEFAULTS` 规则表自动填充实体字段（如 created_at → Utc::now()）。
//! 实体有对应字段则填充，没有则跳过。

use proc_macro::TokenStream;
use quote::quote;
use ryframe_core::auto_fill::{DEFAULTS, FillSource, FillStrategy};
use syn::{Data, DeriveInput, Fields, Ident, parse_macro_input};

// ============================================================
// SkipAttr — 解析 #[auto_fill(field_name, skip)]
// ============================================================

/// 解析 `#[auto_fill(field_name, skip)]`，标记排除字段
///
/// 示例：
/// ```ignore
/// #[derive(AutoFill)]
/// #[auto_fill(login_date, skip)]
/// pub struct Model { ... }
/// ```
struct SkipAttr {
    field: Ident,
}

impl syn::parse::Parse for SkipAttr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let field: Ident = input.parse()?;
        if input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
            let kw: Ident = input.parse()?;
            if kw == "skip" {
                return Ok(SkipAttr { field });
            }
            return Err(syn::Error::new(kw.span(), "expected `skip`"));
        }
        Err(syn::Error::new(field.span(), "expected `, skip`"))
    }
}

// ============================================================
// AutoFill derive 宏 - expand 函数
// ============================================================

pub(crate) fn expand_auto_fill(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // 1. 收集 struct 所有字段名
    let field_names: Vec<String> = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(f) => f
                .named
                .iter()
                .filter_map(|f| f.ident.as_ref().map(|i| i.to_string()))
                .collect(),
            _ => vec![],
        },
        _ => vec![],
    };

    // 2. 收集 skip 列表
    let mut skips = Vec::new();
    for attr in &input.attrs {
        if attr.path().is_ident("auto_fill")
            && let Ok(a) = attr.parse_args::<SkipAttr>()
        {
            skips.push(a.field.to_string());
        }
    }

    // 3. 遍历默认规则，匹配字段生成赋值语句
    let mut insert_stmts: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_stmts: Vec<proc_macro2::TokenStream> = Vec::new();

    for rule in DEFAULTS {
        // 字段在实体中存在 && 不在 skip 列表中
        if field_names.iter().any(|f| f == rule.field_name)
            && !skips.iter().any(|s| s == rule.field_name)
        {
            let field = Ident::new(rule.field_name, proc_macro2::Span::call_site());
            let expr = match rule.source {
                FillSource::Now => quote!(ctx.now),
                FillSource::UserId => quote!(ctx.user_id),
                FillSource::Username => quote!(ctx.username),
            };
            match rule.strategy {
                FillStrategy::Insert => {
                    insert_stmts.push(quote!(self.#field = #expr;));
                }
                FillStrategy::Update => {
                    update_stmts.push(quote!(self.#field = #expr;));
                }
                FillStrategy::All => {
                    insert_stmts.push(quote!(self.#field = #expr;));
                    update_stmts.push(quote!(self.#field = #expr;));
                }
            }
        }
    }

    // 4. 生成 impl
    let expanded = quote! {
        impl ryframe_core::auto_fill::AutoFill for #name {
            fn fill_on_insert(&mut self, ctx: &ryframe_core::auto_fill::FillContext) {
                #(#insert_stmts)*
            }
            fn fill_on_update(&mut self, ctx: &ryframe_core::auto_fill::FillContext) {
                #(#update_stmts)*
            }
        }
    };

    expanded.into()
}
