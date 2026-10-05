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
    let field_descriptors = fields
        .iter()
        .map(field_descriptor)
        .collect::<syn::Result<Vec<_>>>()?;
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
                    fields: &[#(#field_descriptors),*],
                }
            }
        }
    })
}

fn field_descriptor(field: &Field) -> syn::Result<proc_macro2::TokenStream> {
    let field_name = field.ident.as_ref().expect("具名字段").to_string();
    let field_type = &field.ty;
    let column_default = field_name.clone();
    let mut column = None;
    let mut rename_from = None;
    let mut default = None;
    let mut primary_key = false;
    let mut generated = false;
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
    let column = column.unwrap_or(column_default);
    let rename = rename_from
        .map(|value| quote!(Some(#value)))
        .unwrap_or_else(|| quote!(None));
    let default = default
        .map(|value| quote!(Some(#value)))
        .unwrap_or_else(|| quote!(None));
    Ok(quote! {
        ::ryframe_sdk::ResourceFieldDescriptor {
            name: #field_name,
            column: #column,
            rust_type: stringify!(#field_type),
            rename_from: #rename,
            default: #default,
            primary_key: #primary_key,
            generated: #generated,
            read_only: #read_only,
            filter: #filter,
            sort: #sort,
            unique: #unique,
        }
    })
}

fn required(value: Option<String>, input: &DeriveInput, field: &str) -> syn::Result<String> {
    value.ok_or_else(|| syn::Error::new_spanned(input, format!("缺少 resource {field}")))
}
