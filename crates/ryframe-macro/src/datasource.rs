//! 多数据源注解 — 类似 MyBatis-Plus `@DS("db_name")`
//!
//! 标注在 async 函数上，自动将函数体包裹在目标数据源上下文中。
//! 函数返回时自动恢复之前的数据源（支持嵌套）。

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, LitStr, parse_macro_input};

/// 将 `#[datasource("name")]` 标注的函数体包裹到数据源 scope 中。
pub(crate) fn expand_datasource(attr: TokenStream, item: TokenStream) -> TokenStream {
    // 解析数据源名称
    let ds_name: LitStr = parse_macro_input!(attr as LitStr);
    let ds_name_str = ds_name.value();

    // 解析被标注的函数
    let input_fn: ItemFn = parse_macro_input!(item as ItemFn);

    // 拆解函数各部分
    let fn_attrs = &input_fn.attrs;
    let fn_vis = &input_fn.vis;
    let fn_sig = &input_fn.sig;
    let fn_block = &input_fn.block;

    // 生成包裹代码
    let expanded = quote! {
        #(#fn_attrs)*
        #fn_vis #fn_sig {
            ryframe_core::DATA_SOURCE_NAME.scope(
                #ds_name_str.to_string(),
                async move {
                    #fn_block
                }
            ).await
        }
    };

    expanded.into()
}
