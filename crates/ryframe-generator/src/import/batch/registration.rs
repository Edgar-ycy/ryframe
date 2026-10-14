use syn::{Expr, Item, Stmt, Token, Type, punctuated::Punctuated};

use super::error;
use crate::ResourceError;

pub(super) fn register(source: &str, models: &[(String, String)]) -> Result<String, ResourceError> {
    let file =
        syn::parse_file(source).map_err(|parse| error(format!("资源注册表语法错误：{parse}")))?;
    let function = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.sig.ident == "descriptors" => Some(function),
            _ => None,
        })
        .ok_or_else(|| error("未找到 descriptors()；保留显式 vec! 资源描述符列表后重试"))?;
    let [Stmt::Expr(Expr::Macro(expression), None)] = function.block.stmts.as_slice() else {
        return Err(error(
            "descriptors() 必须返回显式 vec! 列表；复杂资源装配请先改为显式列表",
        ));
    };
    if !expression.mac.path.is_ident("vec") {
        return Err(error("descriptors() 必须返回 vec! 列表"));
    }
    let entries = expression
        .mac
        .parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated)
        .map_err(|parse| error(format!("资源描述符列表无效：{parse}")))?;
    let mut declarations = String::new();
    let mut additions = Vec::new();
    for (module, model) in models {
        match file.items.iter().find(|item| match item {
            Item::Mod(item) => item.ident == module,
            Item::Struct(item) => item.ident == module,
            Item::Enum(item) => item.ident == module,
            Item::Type(item) => item.ident == module,
            _ => false,
        }) {
            Some(Item::Mod(item)) if item.content.is_none() && item.attrs.is_empty() => {}
            Some(_) => {
                return Err(error(format!(
                    "导入模块 {module} 与已有 Rust 定义冲突；本批未写入"
                )));
            }
            None => declarations.push_str(&format!("pub mod {module};\n")),
        }
        if !entries.iter().any(|entry| registered(entry, module, model)) {
            additions.push(format!(
                "<{module}::{model} as ryframe_sdk::ResourceModel>::descriptor(),"
            ));
        }
    }
    if additions.is_empty() && declarations.is_empty() {
        return Ok(source.into());
    }
    let position = expression.mac.delimiter.span().close().byte_range().start;
    if position >= source.len() || !source.is_char_boundary(position) {
        return Err(error("无法定位资源注册位置；本批未写入"));
    }
    let comma = if !additions.is_empty() && !entries.is_empty() && !entries.trailing_punct() {
        ","
    } else {
        ""
    };
    Ok(format!(
        "{declarations}{}{comma}\n{}\n{}",
        &source[..position],
        additions.join("\n"),
        &source[position..]
    ))
}

fn registered(entry: &Expr, module: &str, model: &str) -> bool {
    let Expr::Call(call) = entry else {
        return false;
    };
    let Expr::Path(path) = call.func.as_ref() else {
        return false;
    };
    let Some(qualified) = &path.qself else {
        return false;
    };
    let Type::Path(ty) = qualified.ty.as_ref() else {
        return false;
    };
    call.args.is_empty()
        && path
            .path
            .segments
            .last()
            .is_some_and(|item| item.ident == "descriptor")
        && ty
            .path
            .segments
            .iter()
            .map(|item| item.ident.to_string())
            .eq([module, model].map(str::to_owned))
}
