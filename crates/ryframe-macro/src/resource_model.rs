use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Field, Fields, LitStr, parse_macro_input};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_inner(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_inner(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let mut name = None;
    let mut title = None;
    let mut table = None;
    let mut database = None;
    let mut route = None;
    let mut menu_parent = None;
    let mut menu_order = 100_u16;
    let mut primary_key = None;
    for attribute in &input.attrs {
        if !attribute.path().is_ident("resource") {
            continue;
        }
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("title") {
                title = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("table") {
                table = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("database") {
                database = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("route") {
                route = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("menu_parent") {
                menu_parent = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("menu_order") {
                menu_order = meta.value()?.parse::<syn::LitInt>()?.base10_parse()?;
            } else if meta.path.is_ident("primary_key") {
                if primary_key.is_some() {
                    return Err(meta.error("primary_key 只能声明一次"));
                }
                primary_key = Some(meta.value()?.parse::<syn::ExprArray>()?);
            } else {
                return Err(meta.error("不支持的 resource 属性"));
            }
            Ok(())
        })?;
    }
    let name = required(name, input, "name")?;
    let title = title.unwrap_or_else(|| name.clone());
    let table = required(table, input, "table")?;
    let database = match required(database, input, "database")?.as_str() {
        "control" => quote!(::ryframe_sdk::ResourceDatabase::Control),
        "tenant" => quote!(::ryframe_sdk::ResourceDatabase::Tenant),
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "resource database 只支持 control 或 tenant",
            ));
        }
    };
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    input,
                    "ResourceModel 只支持具名字段结构体",
                ));
            }
        },
        _ => return Err(syn::Error::new_spanned(input, "ResourceModel 只支持结构体")),
    };
    let parsed_fields = fields
        .iter()
        .map(field_descriptor)
        .collect::<syn::Result<Vec<_>>>()?;
    let primary_key = ordered_primary_key(primary_key, &parsed_fields, input)?;
    let field_descriptors = parsed_fields.iter().map(|field| &field.tokens);
    let model = &input.ident;
    let route = route
        .map(|value| quote!(Some(#value)))
        .unwrap_or_else(|| quote!(None));
    let menu_parent = menu_parent
        .map(|value| quote!(Some(#value)))
        .unwrap_or_else(|| quote!(None));
    Ok(quote! {
        impl ::ryframe_sdk::ResourceModel for #model {
            fn descriptor() -> ::ryframe_sdk::ResourceDescriptor {
                ::ryframe_sdk::ResourceDescriptor {
                    name: #name,
                    title: #title,
                    table: #table,
                    database: #database,
                    model_module: module_path!(),
                    model_type: stringify!(#model),
                    route: #route,
                    menu_parent: #menu_parent,
                    menu_order: #menu_order,
                    primary_key: &[#(#primary_key),*],
                    fields: &[#(#field_descriptors),*],
                }
            }
        }
    })
}

struct ParsedField {
    name: String,
    primary_key: bool,
    tokens: proc_macro2::TokenStream,
}

fn ordered_primary_key(
    explicit: Option<syn::ExprArray>,
    fields: &[ParsedField],
    input: &DeriveInput,
) -> syn::Result<Vec<String>> {
    let inferred = fields
        .iter()
        .filter(|field| field.primary_key)
        .map(|field| field.name.clone())
        .collect::<Vec<_>>();
    let Some(explicit) = explicit else {
        return Ok(inferred);
    };
    let mut keys = Vec::new();
    for expression in &explicit.elems {
        let syn::Expr::Lit(literal) = expression else {
            return Err(syn::Error::new_spanned(
                expression,
                "primary_key 只接受字段名字串",
            ));
        };
        let syn::Lit::Str(key) = &literal.lit else {
            return Err(syn::Error::new_spanned(
                expression,
                "primary_key 只接受字段名字串",
            ));
        };
        keys.push(key.value());
    }
    let unique = keys.iter().collect::<std::collections::BTreeSet<_>>();
    if keys.is_empty()
        || unique.len() != keys.len()
        || unique != inferred.iter().collect::<std::collections::BTreeSet<_>>()
    {
        return Err(syn::Error::new_spanned(
            input,
            "primary_key 顺序必须恰好包含全部标记为主键的字段，不能重复或遗漏",
        ));
    }
    Ok(keys)
}

fn field_descriptor(field: &Field) -> syn::Result<ParsedField> {
    let field_name = field.ident.as_ref().expect("具名字段").to_string();
    let field_type = &field.ty;
    let column_default = field_name.clone();
    let mut column = None;
    let mut rename_from = None;
    let mut default = None;
    let mut primary_key = false;
    let mut generated = false;
    let mut auto_increment = false;
    let mut read_only = false;
    let mut filter = false;
    let mut sort = false;
    let mut unique = false;
    for attribute in &field.attrs {
        if !attribute.path().is_ident("resource") {
            continue;
        }
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_from") {
                rename_from = Some(meta.value()?.parse::<LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("column") {
                column = Some(meta.value()?.parse::<LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("default") {
                default = Some(meta.value()?.parse::<LitStr>()?.value());
                Ok(())
            } else if meta.path.is_ident("primary_key") {
                primary_key = true;
                Ok(())
            } else if meta.path.is_ident("generated") {
                generated = true;
                Ok(())
            } else if meta.path.is_ident("auto_increment") {
                auto_increment = true;
                generated = true;
                Ok(())
            } else if meta.path.is_ident("read_only") {
                read_only = true;
                Ok(())
            } else if meta.path.is_ident("filter") {
                filter = true;
                Ok(())
            } else if meta.path.is_ident("sort") {
                sort = true;
                Ok(())
            } else if meta.path.is_ident("unique") {
                unique = true;
                Ok(())
            } else {
                Err(meta.error("不支持的字段 resource 属性"))
            }
        })?;
    }
    if auto_increment && (!primary_key || field_name != "id" || default.is_some()) {
        return Err(syn::Error::new_spanned(
            field,
            "auto_increment 仅支持无默认值的 id 主键；声明 #[resource(primary_key, auto_increment)]",
        ));
    }
    let column = column.unwrap_or(column_default);
    let rename = rename_from
        .map(|value| quote!(Some(#value)))
        .unwrap_or_else(|| quote!(None));
    let default = default
        .map(|value| quote!(Some(#value)))
        .unwrap_or_else(|| quote!(None));
    let tokens = quote! {
        ::ryframe_sdk::ResourceFieldDescriptor {
            name: #field_name,
            column: #column,
            rust_type: stringify!(#field_type),
            rename_from: #rename,
            default: #default,
            primary_key: #primary_key,
            generated: #generated,
            auto_increment: #auto_increment,
            read_only: #read_only,
            filter: #filter,
            sort: #sort,
            unique: #unique,
        }
    };
    Ok(ParsedField {
        name: field_name,
        primary_key,
        tokens,
    })
}

fn required(value: Option<String>, input: &DeriveInput, field: &str) -> syn::Result<String> {
    value.ok_or_else(|| syn::Error::new_spanned(input, format!("缺少 resource {field}")))
}

#[cfg(test)]
mod tests {
    #[test]
    fn explicit_primary_key_order_rejects_missing_duplicate_or_non_string_fields() {
        for keys in [
            "[]",
            "[\"id\"]",
            "[\"id\", \"id\"]",
            "[\"tenant_id\", \"other\"]",
            "[1, \"id\"]",
        ] {
            let source = format!(
                "#[resource(name = \"item\", table = \"biz_item\", database = \"tenant\", primary_key = {keys})] struct Item {{ #[resource(primary_key)] id: i64, #[resource(primary_key)] tenant_id: String }}"
            );
            let input = syn::parse_str(&source).unwrap();
            assert!(
                super::expand_inner(&input)
                    .unwrap_err()
                    .to_string()
                    .contains("primary_key")
            );
        }
    }

    #[test]
    fn auto_increment_requires_explicit_id_primary_key_without_default() {
        for field in [
            "#[resource(auto_increment)] id: i64",
            "#[resource(primary_key, auto_increment)] other: i64",
            "#[resource(primary_key, auto_increment, default = \"0\")] id: i64",
        ] {
            let source = format!(
                "#[resource(name = \"item\", table = \"biz_item\", database = \"tenant\")] struct Item {{ {field} }}"
            );
            let input = syn::parse_str(&source).unwrap();
            let error = super::expand_inner(&input).unwrap_err().to_string();
            assert!(error.contains("auto_increment"));
            assert!(error.contains("无默认值"));
        }
    }

    #[test]
    fn auto_increment_descriptor_is_also_service_managed() {
        let input = syn::parse_quote! {
            #[resource(name = "item", table = "biz_item", database = "tenant")]
            struct Item {
                #[resource(primary_key, auto_increment)]
                id: i64,
            }
        };
        let expanded = super::expand_inner(&input).unwrap().to_string();
        assert!(expanded.contains("auto_increment : true"));
        assert!(expanded.contains("generated : true"));
    }
}
