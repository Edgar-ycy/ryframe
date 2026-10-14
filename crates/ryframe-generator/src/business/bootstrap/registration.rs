use syn::{Expr, Item, Stmt, Token, punctuated::Punctuated};

use crate::ResourceError;

pub(super) fn add_module_registration(
    source: &str,
    crate_ident: &str,
) -> Result<String, ResourceError> {
    let file = syn::parse_file(source)
        .map_err(|error| registration_error(format!("业务注册表语法错误：{error}")))?;
    let function = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.sig.ident == "business_modules" => Some(function),
            _ => None,
        })
        .ok_or_else(|| registration_error("未找到 business_modules 注册函数"))?;
    let [Stmt::Expr(Expr::Macro(expression), None)] = function.block.stmts.as_slice() else {
        return Err(registration_error(
            "business_modules 必须返回显式 vec! 注册表；复杂装配请手工登记业务模块",
        ));
    };
    if !expression.mac.path.is_ident("vec") {
        return Err(registration_error("business_modules 没有返回 vec! 注册表"));
    }
    let entries = expression
        .mac
        .parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated)
        .map_err(|error| registration_error(format!("业务模块列表无效：{error}")))?;
    if entries
        .iter()
        .any(|entry| registered_module(entry, crate_ident))
    {
        return Err(registration_error(
            "业务模块已在组合根注册，请选择新的模块名",
        ));
    }
    let position = insertion_offset(expression.mac.delimiter.span().close(), source)?;
    let comma = if !entries.is_empty() && !entries.trailing_punct() {
        ","
    } else {
        ""
    };
    Ok(format!(
        "{}{comma}\n{crate_ident}::module(),\n{}",
        &source[..position],
        &source[position..]
    ))
}

fn registered_module(entry: &Expr, crate_ident: &str) -> bool {
    let Expr::Call(call) = entry else {
        return false;
    };
    let Expr::Path(path) = call.func.as_ref() else {
        return false;
    };
    call.args.is_empty()
        && path
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .eq([crate_ident, "module"].map(str::to_owned))
}

fn insertion_offset(span: proc_macro2::Span, source: &str) -> Result<usize, ResourceError> {
    let position = span.byte_range().start;
    if !source.is_char_boundary(position) || position >= source.len() {
        return Err(registration_error(
            "无法定位业务列表闭合位置，正式源码尚未修改",
        ));
    }
    Ok(position)
}

fn registration_error(message: impl Into<String>) -> ResourceError {
    ResourceError::new(
        message,
        "检查 crates/ryframe/src/business.rs 的 business_modules；保留显式模块列表后重试",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_multiline_and_commented_registries_preserve_source() {
        for list in [
            "vec![]",
            "vec![first::module()]",
            "vec![\n// 已有模块\nfirst::module(),\n]",
        ] {
            let source = format!(
                "// 中文说明与无关文本 \"second::module(),\"\npub fn business_modules() -> Vec<Module> {{ {list} }}\nfn unrelated() {{ let _ = vec![1, 2]; }}"
            );
            let updated = add_module_registration(&source, "second").unwrap();
            syn::parse_file(&updated).unwrap();
            assert!(updated.starts_with("// 中文说明与无关文本"));
            assert!(updated.ends_with("fn unrelated() { let _ = vec![1, 2]; }"));
            assert!(
                add_module_registration(&updated, "second")
                    .unwrap_err()
                    .to_string()
                    .contains("已在组合根注册")
            );
        }
    }
}
