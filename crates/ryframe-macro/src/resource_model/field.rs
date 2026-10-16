use quote::quote;
use syn::{Field, meta::ParseNestedMeta};

use super::{ParsedField, optional_string, parse_single_string};

#[derive(Default)]
struct Attributes {
    column: Option<String>,
    rename_from: Option<String>,
    default: Option<String>,
    sql_type: Option<String>,
    collation: Option<String>,
    sql_default: Option<String>,
    sql_on_update: Option<String>,
    sql_comment: Option<String>,
    primary_key: bool,
    generated: bool,
    auto_increment: bool,
    read_only: bool,
    filter: bool,
    sort: bool,
    unique: bool,
}

pub(super) fn parse(field: &Field) -> syn::Result<ParsedField> {
    let name = field.ident.as_ref().expect("具名字段").to_string();
    let mut attributes = Attributes::default();
    for attribute in &field.attrs {
        if attribute.path().is_ident("resource") {
            attribute.parse_nested_meta(|meta| attributes.parse(&meta))?;
        }
    }
    if attributes.auto_increment
        && (!attributes.primary_key
            || name != "id"
            || attributes.default.is_some()
            || attributes.sql_default.is_some()
            || attributes.sql_on_update.is_some())
    {
        return Err(syn::Error::new_spanned(
            field,
            "auto_increment 仅支持无默认值或更新表达式的 id 主键；声明 #[resource(primary_key, auto_increment)]",
        ));
    }
    Ok(ParsedField {
        primary_key: attributes.primary_key,
        tokens: attributes.tokens(&name, &field.ty),
        name,
    })
}

impl Attributes {
    fn parse(&mut self, meta: &ParseNestedMeta<'_>) -> syn::Result<()> {
        let name = meta
            .path
            .get_ident()
            .map(ToString::to_string)
            .unwrap_or_default();
        let literal = match name.as_str() {
            "column" => Some(&mut self.column),
            "rename_from" => Some(&mut self.rename_from),
            "default" => Some(&mut self.default),
            "sql_type" => Some(&mut self.sql_type),
            "collation" => Some(&mut self.collation),
            "sql_default" => Some(&mut self.sql_default),
            "sql_on_update" => Some(&mut self.sql_on_update),
            "sql_comment" => Some(&mut self.sql_comment),
            _ => None,
        };
        if let Some(literal) = literal {
            return parse_single_string(meta, literal, &name);
        }
        match name.as_str() {
            "primary_key" => self.primary_key = true,
            "generated" => self.generated = true,
            "auto_increment" => {
                self.auto_increment = true;
                self.generated = true;
            }
            "read_only" => self.read_only = true,
            "filter" => self.filter = true,
            "sort" => self.sort = true,
            "unique" => self.unique = true,
            _ => return Err(meta.error("不支持的字段 resource 属性")),
        }
        Ok(())
    }

    fn tokens(self, field_name: &str, field_type: &syn::Type) -> proc_macro2::TokenStream {
        let column = self.column.unwrap_or_else(|| field_name.to_owned());
        let rename = optional_string(self.rename_from);
        let default = optional_string(self.default);
        let sql_type = optional_string(self.sql_type);
        let collation = optional_string(self.collation);
        let sql_default = optional_string(self.sql_default);
        let sql_on_update = optional_string(self.sql_on_update);
        let sql_comment = optional_string(self.sql_comment);
        let primary_key = self.primary_key;
        let generated = self.generated;
        let auto_increment = self.auto_increment;
        let read_only = self.read_only;
        let filter = self.filter;
        let sort = self.sort;
        let unique = self.unique;
        quote! {
            ::ryframe_sdk::ResourceFieldDescriptor {
                name: #field_name, column: #column, rust_type: stringify!(#field_type),
                sql_type: #sql_type, collation: #collation,
                sql_default: #sql_default, sql_on_update: #sql_on_update,
                sql_comment: #sql_comment,
                rename_from: #rename, default: #default,
                primary_key: #primary_key, generated: #generated, auto_increment: #auto_increment,
                read_only: #read_only, filter: #filter, sort: #sort, unique: #unique,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn database_expression_attributes_are_separate_from_literal_defaults() {
        let input = syn::parse_quote! {
            #[resource(name="item",table="table1",database="control")]
            struct Item {
                #[resource(primary_key)] id: i64,
                #[resource(sql_default="CURRENT_TIMESTAMP(6)",sql_on_update="CURRENT_TIMESTAMP(6)")]
                updated_at: chrono::DateTime<chrono::Utc>,
            }
        };
        let output = super::super::expand_inner(&input).unwrap().to_string();
        assert!(output.contains("sql_default : Some (\"CURRENT_TIMESTAMP(6)\")"));
        assert!(output.contains("sql_on_update : Some (\"CURRENT_TIMESTAMP(6)\")"));
        let input = syn::parse_quote! {
            #[resource(name="item",table="table1",database="control")]
            struct Item { #[resource(primary_key,default="1",default="2")] id: i64 }
        };
        assert!(
            super::super::expand_inner(&input)
                .unwrap_err()
                .to_string()
                .contains("default 只能声明一次")
        );
    }

    #[test]
    fn column_comments_are_exact_optional_metadata_and_cannot_be_duplicated() {
        let input = syn::parse_quote! {
            #[resource(name="item",table="table1",database="control")]
            struct Item { #[resource(primary_key,sql_comment="中文 '单引号'\\路径")] id: i64 }
        };
        let output = super::super::expand_inner(&input).unwrap().to_string();
        assert!(output.contains("sql_comment : Some (\"中文 '单引号'\\\\路径\")"));
        let input = syn::parse_quote! {
            #[resource(name="item",table="table1",database="control")]
            struct Item { #[resource(primary_key,sql_comment="一",sql_comment="二")] id: i64 }
        };
        assert!(
            super::super::expand_inner(&input)
                .unwrap_err()
                .to_string()
                .contains("sql_comment 只能声明一次")
        );
    }
}
