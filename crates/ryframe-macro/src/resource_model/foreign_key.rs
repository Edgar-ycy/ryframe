use quote::{format_ident, quote};
use syn::{LitStr, meta::ParseNestedMeta};

pub(super) struct ForeignKey {
    name: String,
    columns: Vec<String>,
    referenced_table: String,
    referenced_columns: Vec<String>,
    update_rule: String,
    delete_rule: String,
    match_option: String,
}

fn columns(meta: &ParseNestedMeta<'_>) -> syn::Result<Vec<String>> {
    meta.value()?
        .parse::<syn::ExprArray>()?
        .elems
        .iter()
        .map(|value| syn::parse2::<LitStr>(quote!(#value)).map(|value| value.value()))
        .collect()
}

pub(super) fn parse(meta: &ParseNestedMeta<'_>) -> syn::Result<ForeignKey> {
    let mut name = None;
    let mut local = None;
    let mut table = None;
    let mut referenced = None;
    let mut update = None;
    let mut delete = None;
    let mut matching = None;
    meta.parse_nested_meta(|property| {
        if property.path.is_ident("name") {
            super::parse_single_string(&property, &mut name, "外键 name")?;
        } else if property.path.is_ident("referenced_table") {
            super::parse_single_string(&property, &mut table, "外键 referenced_table")?;
        } else if property.path.is_ident("update_rule") {
            super::parse_single_string(&property, &mut update, "外键 update_rule")?;
        } else if property.path.is_ident("delete_rule") {
            super::parse_single_string(&property, &mut delete, "外键 delete_rule")?;
        } else if property.path.is_ident("match_option") {
            super::parse_single_string(&property, &mut matching, "外键 match_option")?;
        } else if property.path.is_ident("columns") && local.is_none() {
            local = Some(columns(&property)?);
        } else if property.path.is_ident("referenced_columns") && referenced.is_none() {
            referenced = Some(columns(&property)?);
        } else {
            return Err(property.error("外键字段未知或重复；请声明完整有序物理约束"));
        }
        Ok(())
    })?;
    Ok(ForeignKey {
        name: name.ok_or_else(|| meta.error("外键缺少 name"))?,
        columns: local.ok_or_else(|| meta.error("外键缺少 columns"))?,
        referenced_table: table.ok_or_else(|| meta.error("外键缺少 referenced_table"))?,
        referenced_columns: referenced.ok_or_else(|| meta.error("外键缺少 referenced_columns"))?,
        update_rule: update.ok_or_else(|| meta.error("外键缺少 update_rule"))?,
        delete_rule: delete.ok_or_else(|| meta.error("外键缺少 delete_rule"))?,
        match_option: matching.ok_or_else(|| meta.error("外键缺少 match_option"))?,
    })
}

fn action(value: &str) -> syn::Result<proc_macro2::TokenStream> {
    let variant = match value {
        "CASCADE" => "Cascade",
        "RESTRICT" => "Restrict",
        "SET NULL" => "SetNull",
        "NO ACTION" => "NoAction",
        _ => {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "外键引用动作只支持 CASCADE/RESTRICT/SET NULL/NO ACTION",
            ));
        }
    };
    let variant = format_ident!("{variant}");
    Ok(quote!(::ryframe_sdk::ResourceForeignKeyAction::#variant))
}

pub(super) fn tokens(keys: &[ForeignKey]) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    keys.iter()
        .map(|key| {
            let name = &key.name;
            let columns = &key.columns;
            let table = &key.referenced_table;
            let referenced = &key.referenced_columns;
            if key.match_option != "NONE" {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "物理外键只支持 MATCH NONE",
                ));
            }
            let update = action(&key.update_rule)?;
            let delete = action(&key.delete_rule)?;
            Ok(quote!(::ryframe_sdk::ResourceForeignKeyDescriptor {
                name: #name, columns: &[#(#columns),*], referenced_table: #table,
                referenced_columns: &[#(#referenced),*], update_rule: #update,
                delete_rule: #delete, match_option: ::ryframe_sdk::ResourceForeignKeyMatch::None,
            }))
        })
        .collect()
}
