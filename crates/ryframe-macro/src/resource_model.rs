use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Field, Fields, LitStr, parse_macro_input};

mod field;
mod index;

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
    let mut collation = None;
    let mut database = None;
    let mut route = None;
    let mut menu_parent = None;
    let mut menu_order = 100_u16;
    let mut primary_key = None;
    let mut indexes = Vec::new();
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
            } else if meta.path.is_ident("collation") {
                parse_single_string(&meta, &mut collation, "collation")?;
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
            } else if meta.path.is_ident("index") {
                indexes.push(index::parse(&meta)?);
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
    let fields = named_fields(input)?;
    let parsed_fields = fields
        .iter()
        .map(field::parse)
        .collect::<syn::Result<Vec<_>>>()?;
    let primary_key = ordered_primary_key(primary_key, &parsed_fields, input)?;
    let indexes = index::tokens(&indexes, &parsed_fields, input)?;
    let field_descriptors = parsed_fields.iter().map(|field| &field.tokens);
    let model = &input.ident;
    let route = optional_string(route);
    let menu_parent = optional_string(menu_parent);
    let collation = optional_string(collation);
    Ok(quote! {
        impl ::ryframe_sdk::ResourceModel for #model {
            fn descriptor() -> ::ryframe_sdk::ResourceDescriptor {
                ::ryframe_sdk::ResourceDescriptor {
                    name: #name,
                    title: #title,
                    table: #table,
                    collation: #collation,
                    database: #database,
                    model_module: module_path!(),
                    model_type: stringify!(#model),
                    route: #route,
                    menu_parent: #menu_parent,
                    menu_order: #menu_order,
                    primary_key: &[#(#primary_key),*],
                    indexes: &[#(#indexes),*],
                    fields: &[#(#field_descriptors),*],
                }
            }
        }
    })
}

fn named_fields(
    input: &DeriveInput,
) -> syn::Result<&syn::punctuated::Punctuated<Field, syn::token::Comma>> {
    Ok(match &input.data {
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

fn optional_string(value: Option<String>) -> proc_macro2::TokenStream {
    value
        .map(|value| quote!(Some(#value)))
        .unwrap_or_else(|| quote!(None))
}

fn parse_single_string(
    meta: &syn::meta::ParseNestedMeta<'_>,
    value: &mut Option<String>,
    name: &str,
) -> syn::Result<()> {
    if value.is_some() {
        return Err(meta.error(format!("{name} 只能声明一次")));
    }
    *value = Some(meta.value()?.parse::<LitStr>()?.value());
    Ok(())
}

fn required(value: Option<String>, input: &DeriveInput, field: &str) -> syn::Result<String> {
    value.ok_or_else(|| syn::Error::new_spanned(input, format!("缺少 resource {field}")))
}

#[cfg(test)]
mod tests {
    #[test]
    fn table_and_column_collations_are_distinct_and_duplicates_fail() {
        let input = syn::parse_quote! {
            #[resource(name="item",table="biz_item",database="control",collation="utf8mb4_0900_ai_ci")]
            struct Item { #[resource(primary_key)] id: i64, #[resource(collation="utf8mb4_bin")] code: String }
        };
        let output = super::expand_inner(&input).unwrap().to_string();
        assert!(output.contains("collation : Some (\"utf8mb4_0900_ai_ci\")"));
        assert!(output.contains("collation : Some (\"utf8mb4_bin\")"));
        let input = syn::parse_quote! {
            #[resource(name="item",table="biz_item",database="control",collation="utf8mb4_bin",collation="utf8mb4_general_ci")]
            struct Item { #[resource(primary_key)] id: i64 }
        };
        assert!(
            super::expand_inner(&input)
                .unwrap_err()
                .to_string()
                .contains("collation 只能声明一次")
        );
    }

    #[test]
    fn physical_type_is_preserved_and_duplicate_declarations_fail() {
        let input = syn::parse_quote! {
            #[resource(name = "item", table = "biz_item", database = "control")]
            struct Item {
                #[resource(primary_key)] id: i64,
                #[resource(sql_type = "varchar(32)")] code: String,
            }
        };
        assert!(
            super::expand_inner(&input)
                .unwrap()
                .to_string()
                .contains("sql_type : Some (\"varchar(32)\")")
        );
        let input = syn::parse_quote! {
            #[resource(name = "item", table = "biz_item", database = "control")]
            struct Item {
                #[resource(primary_key, sql_type = "BIGINT", sql_type = "INT")] id: i64,
            }
        };
        assert!(
            super::expand_inner(&input)
                .unwrap_err()
                .to_string()
                .contains("sql_type 只能声明一次")
        );
    }

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
