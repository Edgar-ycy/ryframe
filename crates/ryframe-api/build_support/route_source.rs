//! 构建脚本的 Rust 路由源码解析辅助逻辑。

use std::{
    collections::BTreeSet,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use syn::{Attribute, Expr, ExprLit, Item, Lit, LitStr, Meta, Token, punctuated::Punctuated};

use super::model::{CompiledRoute, SUPPORTED_HTTP_METHODS};

pub(super) fn collect_rust_files(
    root: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

pub(super) fn collect_routes(
    items: &[Item],
    routes: &mut Vec<CompiledRoute>,
    compiled_handlers: &mut BTreeSet<(String, String)>,
    source: &str,
    module_path: &str,
) -> syn::Result<()> {
    for item in items {
        match item {
            Item::Fn(function) => {
                let handler = qualified_handler(module_path, &function.sig.ident.to_string());
                compiled_handlers.insert((source.to_owned(), handler.clone()));
                let (permission, capability) = collect_access_attributes(&function.attrs)?;
                let route_method = route_attribute_method(&function.attrs)?;
                let documented = documented_route(&function.attrs)?;
                if route_method.is_some() || permission.is_some() || capability.is_some() {
                    let Some((documented_method, documented_path)) = documented else {
                        return Err(syn::Error::new_spanned(
                            function,
                            "HTTP 路由必须声明唯一的 #[utoipa::path(...)] 契约",
                        ));
                    };
                    if let Some(route_method) = route_method
                        && route_method != documented_method
                    {
                        return Err(syn::Error::new_spanned(
                            function,
                            format!(
                                "路由属性方法 {route_method} 与 utoipa 方法 {documented_method} 不一致"
                            ),
                        ));
                    }
                    routes.push(CompiledRoute {
                        source: source.to_owned(),
                        handler,
                        method: documented_method,
                        path: documented_path,
                        permission,
                        capability,
                        declared_policy: None,
                    });
                } else if let Some((method, path)) = documented {
                    routes.push(CompiledRoute {
                        source: source.to_owned(),
                        handler,
                        method,
                        path,
                        permission: None,
                        capability: None,
                        declared_policy: None,
                    });
                }
            }
            Item::Mod(module) => {
                let _ = collect_access_attributes(&module.attrs)?;
                if let Some((_, nested_items)) = &module.content {
                    let nested_path = qualified_handler(module_path, &module.ident.to_string());
                    collect_routes(
                        nested_items,
                        routes,
                        compiled_handlers,
                        source,
                        &nested_path,
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn qualified_handler(module_path: &str, name: &str) -> String {
    if module_path.is_empty() {
        name.to_owned()
    } else {
        format!("{module_path}::{name}")
    }
}

fn collect_access_attributes(
    attributes: &[Attribute],
) -> syn::Result<(Option<String>, Option<String>)> {
    let mut permission = None;
    let mut capability = None;
    for attribute in attributes {
        let marker = attribute
            .path()
            .segments
            .last()
            .map(|segment| segment.ident.to_string());
        if marker.as_deref() != Some("perm") && marker.as_deref() != Some("capability") {
            continue;
        }
        let literal = attribute.parse_args::<LitStr>()?;
        let value = literal.value();
        if value.trim() != value || value.is_empty() {
            return Err(syn::Error::new_spanned(
                attribute,
                "访问标记不得为空或包含首尾空白",
            ));
        }
        let slot = if marker.as_deref() == Some("perm") {
            &mut permission
        } else {
            &mut capability
        };
        if slot.replace(value).is_some() {
            return Err(syn::Error::new_spanned(
                attribute,
                "每个路由最多声明一个同类访问标记",
            ));
        }
    }
    Ok((permission, capability))
}

fn route_attribute_method(attributes: &[Attribute]) -> syn::Result<Option<String>> {
    let mut method = None;
    for attribute in attributes {
        let Some(candidate) = attribute
            .path()
            .segments
            .last()
            .map(|segment| segment.ident.to_string().to_ascii_uppercase())
            .filter(|candidate| SUPPORTED_HTTP_METHODS.contains(&candidate.as_str()))
        else {
            continue;
        };
        let paths = attribute.parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?;
        if paths.len() != 1 {
            return Err(syn::Error::new_spanned(
                attribute,
                "每个处理函数必须声明且只能声明一个 HTTP 路径",
            ));
        }
        let path = paths.first().expect("已校验路由路径数量").value();
        if !path.starts_with('/') || path.chars().any(char::is_whitespace) {
            return Err(syn::Error::new_spanned(
                attribute,
                "HTTP 路由属性必须使用无空白的绝对路径",
            ));
        }
        if method.replace(candidate).is_some() {
            return Err(syn::Error::new_spanned(
                attribute,
                "每个处理函数只能声明一个 HTTP 路由属性",
            ));
        }
    }
    Ok(method)
}

fn documented_route(attributes: &[Attribute]) -> syn::Result<Option<(String, String)>> {
    let mut documented = None;
    for attribute in attributes {
        let segments = &attribute.path().segments;
        if segments.len() != 2
            || segments
                .first()
                .is_none_or(|segment| segment.ident != "utoipa")
            || segments
                .last()
                .is_none_or(|segment| segment.ident != "path")
        {
            continue;
        }
        let entries = attribute.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        let mut method = None;
        let mut path = None;
        for entry in entries {
            match entry {
                Meta::Path(candidate) => {
                    let Some(candidate) = candidate
                        .get_ident()
                        .map(|ident| ident.to_string().to_ascii_uppercase())
                        .filter(|candidate| SUPPORTED_HTTP_METHODS.contains(&candidate.as_str()))
                    else {
                        continue;
                    };
                    if method.replace(candidate).is_some() {
                        return Err(syn::Error::new_spanned(
                            attribute,
                            "utoipa 路由只能声明一个 HTTP 方法",
                        ));
                    }
                }
                Meta::NameValue(entry) if entry.path.is_ident("path") => {
                    let Expr::Lit(ExprLit {
                        lit: Lit::Str(value),
                        ..
                    }) = &entry.value
                    else {
                        return Err(syn::Error::new_spanned(
                            entry.value,
                            "utoipa 路径必须是字符串字面量",
                        ));
                    };
                    if path.replace(value.value()).is_some() {
                        return Err(syn::Error::new_spanned(
                            value,
                            "utoipa 路由只能声明一个 path",
                        ));
                    }
                }
                _ => {}
            }
        }
        let method = method.ok_or_else(|| {
            syn::Error::new_spanned(attribute, "utoipa 路由必须声明受支持的 HTTP 方法")
        })?;
        let path =
            path.ok_or_else(|| syn::Error::new_spanned(attribute, "utoipa 路由必须声明显式 path"))?;
        if documented.replace((method, path)).is_some() {
            return Err(syn::Error::new_spanned(
                attribute,
                "每个处理函数只能声明一个 utoipa 路由",
            ));
        }
    }
    Ok(documented)
}
