//! ryframe-macro — 属性宏 crate
//!
//! 提供 `#[datasource("name")]` 注解，类似 MyBatis-Plus `@DS("db_name")`。

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, LitStr, parse_macro_input};

/// 多数据源注解 — 类似 MyBatis-Plus `@DS("db_name")`
///
/// 标注在 async 函数上，自动将函数体包裹在目标数据源上下文中。
/// 函数返回时自动恢复之前的数据源（支持嵌套）。
///
/// # 标注位置
///
/// - **Handler 层**：直接标注在 axum handler 函数上
/// - **Service 层**：标注在 `impl Service` 的具体方法上
///
/// # 展开原理
///
/// `#[datasource("db_device")]` 展开为：
/// ```
/// // ryframe_core::DATA_SOURCE_NAME.scope("db_device".to_string(), async move {
/// //     // 原始函数体
/// // }).await
/// ```
///
/// `task_local!.scope()` 自动处理嵌套和恢复。
///
/// # 示例
///
/// ```
/// // use ryframe_macro::datasource;
///
/// // impl DeviceServiceImpl {
/// //     /// 从设备库查询 — 只需注解，无需传 db 参数
/// //     #[datasource("db_device")]
/// //     pub async fn list_devices(&self, query: PageQuery) -> AppResult<PageResult<DeviceVo>> {
/// //         let db = self.device_repo.db(); // ← 自动解析为 db_device 连接
/// //         self.device_repo.find_by_page(&db, query).await
/// //     }
/// // }
/// ```
#[proc_macro_attribute]
pub fn datasource(attr: TokenStream, item: TokenStream) -> TokenStream {
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

    // 生成包裹代码：将原始函数体塞进 DATA_SOURCE_NAME.scope() 中
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
