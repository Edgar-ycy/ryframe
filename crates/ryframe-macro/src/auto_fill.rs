//! 自动填充 derive 宏
//!
//! 按 `DEFAULTS` 规则表自动填充实体字段（如 created_at → Utc::now()）。
//! 实体有对应字段则填充，没有则跳过。
//!
//! 支持两种标注方式：
//! - 字段级 `#[auto_fill(snowflake)]` / `#[auto_fill(skip)]`（推荐）
//! - struct 级 `#[auto_fill(field_name, skip)]` / `#[auto_fill(field_name, snowflake)]`（兼容）

use proc_macro::TokenStream;
use quote::quote;
use ryframe_core::auto_fill::{DEFAULTS, FillSource, FillStrategy};
use syn::{Data, DeriveInput, Fields, Ident, parse_macro_input};

// ============================================================
// AutoFillAction — 标注动作
// ============================================================

enum AutoFillAction {
    Skip,
    Snowflake,
}

// ============================================================
// FieldAction — 解析字段级 #[auto_fill(skip)] / #[auto_fill(snowflake)]
// ============================================================

struct FieldAction {
    action: AutoFillAction,
}

impl syn::parse::Parse for FieldAction {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let kw: Ident = input.parse()?;
        match kw.to_string().as_str() {
            "skip" => Ok(FieldAction {
                action: AutoFillAction::Skip,
            }),
            "snowflake" => Ok(FieldAction {
                action: AutoFillAction::Snowflake,
            }),
            _ => Err(syn::Error::new(kw.span(), "expected `skip` or `snowflake`")),
        }
    }
}

// ============================================================
// StructAttr — 解析 struct 级 #[auto_fill(field_name, skip|snowflake)]
// ============================================================

struct StructAttr {
    field: Ident,
    action: AutoFillAction,
}

impl syn::parse::Parse for StructAttr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let field: Ident = input.parse()?;
        if input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
            let kw: Ident = input.parse()?;
            match kw.to_string().as_str() {
                "skip" => Ok(StructAttr {
                    field,
                    action: AutoFillAction::Skip,
                }),
                "snowflake" => Ok(StructAttr {
                    field,
                    action: AutoFillAction::Snowflake,
                }),
                _ => Err(syn::Error::new(kw.span(), "expected `skip` or `snowflake`")),
            }
        } else {
            Err(syn::Error::new(
                field.span(),
                "expected `, skip` or `, snowflake`",
            ))
        }
    }
}

// ============================================================
// AutoFill derive 宏 - expand 函数
// ============================================================

pub(crate) fn expand_auto_fill(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // 1. 收集 struct 字段信息：字段名 + 字段级属性
    let mut field_names: Vec<String> = Vec::new();
    let mut field_skips: Vec<String> = Vec::new();
    let mut field_snowflakes: Vec<String> = Vec::new();

    if let Data::Struct(s) = &input.data
        && let Fields::Named(f) = &s.fields
    {
        for field in &f.named {
            let fname = field
                .ident
                .as_ref()
                .map(|i| i.to_string())
                .unwrap_or_default();
            field_names.push(fname.clone());

            // 提取字段上的 #[auto_fill(skip)] / #[auto_fill(snowflake)]
            for attr in &field.attrs {
                if attr.path().is_ident("auto_fill")
                    && let Ok(a) = attr.parse_args::<FieldAction>()
                {
                    match a.action {
                        AutoFillAction::Skip => field_skips.push(fname.clone()),
                        AutoFillAction::Snowflake => field_snowflakes.push(fname.clone()),
                    }
                }
            }
        }
    }

    // 2. 收集 struct 级 #[auto_fill(field, skip)] / #[auto_fill(field, snowflake)]
    let mut skips = field_skips;
    let mut snowflakes = field_snowflakes;
    for attr in &input.attrs {
        if attr.path().is_ident("auto_fill")
            && let Ok(a) = attr.parse_args::<StructAttr>()
        {
            match a.action {
                AutoFillAction::Skip => skips.push(a.field.to_string()),
                AutoFillAction::Snowflake => snowflakes.push(a.field.to_string()),
            }
        }
    }

    // 3. 遍历默认规则，匹配字段生成赋值语句
    let mut insert_stmts: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut update_stmts: Vec<proc_macro2::TokenStream> = Vec::new();

    for rule in DEFAULTS {
        if field_names.iter().any(|f| f == rule.field_name)
            && !skips.iter().any(|s| s == rule.field_name)
        {
            let field = Ident::new(rule.field_name, proc_macro2::Span::call_site());
            let expr = match rule.source {
                FillSource::Now => quote!(ctx.now),
                FillSource::UserId => quote!(ctx.user_id),
                FillSource::Username => quote!(ctx.username),
                FillSource::Snowflake => {
                    quote!(ryframe_common::utils::snowflake::next_snowflake_id())
                }
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

    // 4. 为标记了 snowflake 的字段生成插入时的雪花 ID 赋值
    //    主键 ID 只在插入时填充，更新时不填充
    for field_name in &snowflakes {
        if field_names.iter().any(|f| f == field_name) {
            let field = Ident::new(field_name, proc_macro2::Span::call_site());
            insert_stmts.push(quote! {
                self.#field = ryframe_common::utils::snowflake::next_snowflake_id();
            });
        }
    }

    // 5. 生成 impl
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
