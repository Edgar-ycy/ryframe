use std::collections::BTreeSet;

use quote::quote;
use syn::{DeriveInput, LitStr, meta::ParseNestedMeta};

pub(super) struct Index {
    name: String,
    fields: Vec<String>,
    unique: bool,
}

pub(super) fn parse(meta: &ParseNestedMeta<'_>) -> syn::Result<Index> {
    let mut name = None;
    let mut fields = None;
    let mut unique = false;
    meta.parse_nested_meta(|property| {
        if property.path.is_ident("name") {
            super::parse_single_string(&property, &mut name, "索引 name")?;
        } else if property.path.is_ident("fields") {
            if fields.is_some() {
                return Err(property.error("索引 fields 只能声明一次"));
            }
            let array = property.value()?.parse::<syn::ExprArray>()?;
            fields = Some(
                array
                    .elems
                    .iter()
                    .map(|expression| {
                        syn::parse2::<LitStr>(quote!(#expression)).map(|value| value.value())
                    })
                    .collect::<syn::Result<Vec<_>>>()?,
            );
        } else if property.path.is_ident("unique") && !unique {
            unique = true;
        } else {
            return Err(property.error("索引只支持 name、fields 和单次 unique 标志"));
        }
        Ok(())
    })?;
    Ok(Index {
        name: name.ok_or_else(|| meta.error("索引缺少 name"))?,
        fields: fields.ok_or_else(|| meta.error("索引缺少 fields"))?,
        unique,
    })
}

pub(super) fn tokens(
    indexes: &[Index],
    fields: &[super::ParsedField],
    input: &DeriveInput,
) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    let mut names = BTreeSet::new();
    indexes.iter().map(|index| {
        let name = &index.name;
        let keys = &index.fields;
        let unique = index.unique;
        if name.is_empty() || name.eq_ignore_ascii_case("PRIMARY") || !names.insert(name.to_ascii_lowercase()) {
            return Err(syn::Error::new_spanned(input, "索引名称不能为空、重复或使用 PRIMARY；主键由 primary_key 声明"));
        }
        let distinct = keys.iter().collect::<BTreeSet<_>>();
        if keys.is_empty() || distinct.len() != keys.len() || keys.iter().any(|key| !fields.iter().any(|field| field.name == *key)) {
            return Err(syn::Error::new_spanned(input, "索引 fields 必须非空、不重复且引用已有模型字段"));
        }
        Ok(quote!(::ryframe_sdk::ResourceIndexDescriptor { name: #name, fields: &[#(#keys),*], unique: #unique }))
    }).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn preserves_order_and_uniqueness_and_rejects_invalid_indexes() {
        let input = syn::parse_quote! {
            #[resource(name="item",table="table1",database="control",index(name="uk_code_id",fields=["code","id"],unique))]
            struct Item { #[resource(primary_key)] id: i64, code: String }
        };
        let output = super::super::expand_inner(&input).unwrap().to_string();
        assert!(output.contains("name : \"uk_code_id\""));
        assert!(output.contains("fields : & [\"code\" , \"id\"]"));
        assert!(output.contains("unique : true"));
        for declaration in [
            "index(name=\"PRIMARY\",fields=[\"id\"])",
            "index(name=\"idx\",fields=[])",
            "index(name=\"idx\",fields=[\"missing\"])",
            "index(name=\"idx\",fields=[\"id\",\"id\"])",
            "index(name=\"idx\",fields=[\"id\"]),index(name=\"IDX\",fields=[\"code\"])",
        ] {
            let source = format!(
                "#[resource(name=\"item\",table=\"table1\",database=\"control\",{declaration})] struct Item {{ #[resource(primary_key)] id: i64, code: String }}"
            );
            let input = syn::parse_str(&source).unwrap();
            assert!(super::super::expand_inner(&input).is_err(), "{declaration}");
        }
    }
}
