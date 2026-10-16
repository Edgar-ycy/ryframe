use ryframe_generator::{ResourceSpec, normalize_resource, render_resources};

fn spec(comment: &str) -> ResourceSpec {
    let mut spec =
        ResourceSpec::parse(include_str!("../fixtures/order.toml"), "order.toml").unwrap();
    spec.fields
        .iter_mut()
        .find(|field| field.name == "name")
        .unwrap()
        .sql_comment = Some(comment.into());
    spec
}

#[test]
fn sql_comments_preserve_escaping_model_hashes_and_rust_literal_boundaries() {
    for comment in [
        "中文 '注释'",
        "路径\\保留\n换行",
        "\"#; DROP TABLE table1; --",
    ] {
        let resource = normalize_resource(spec(comment), "order.toml", "comments").unwrap();
        let output = render_resources(std::slice::from_ref(&resource)).unwrap();
        let migration = &output
            .assets
            .iter()
            .find(|asset| asset.path.ends_with("order/migration.rs"))
            .unwrap()
            .content;
        let parsed = syn::parse_file(migration).expect("注释中的引号不能结束 Rust 字符串");
        let sql = parsed
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Const(item) if item.ident == "CREATE_TABLE_DDL" => {
                    match item.expr.as_ref() {
                        syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(sql),
                            ..
                        }) => Some(sql.value()),
                        _ => None,
                    }
                }
                _ => None,
            })
            .unwrap();
        assert!(sql.contains(" COMMENT "));
        assert!(sql.contains(&format!(
            "'{}'",
            comment.replace('\\', "\\\\").replace('\'', "''")
        )));
        let catalog = &output
            .assets
            .iter()
            .find(|asset| asset.path == "crates/ryframe-tenant-db/src/generated/catalog.rs")
            .unwrap()
            .content;
        let plain = normalize_resource(spec(""), "order.toml", "empty").unwrap();
        assert_ne!(plain.schema_hash, resource.schema_hash);
        let plain = render_resources(&[plain]).unwrap();
        let plain_catalog = &plain
            .assets
            .iter()
            .find(|asset| asset.path == "crates/ryframe-tenant-db/src/generated/catalog.rs")
            .unwrap()
            .content;
        assert_eq!(catalog, plain_catalog, "注释不改变结构目录与结构指纹");
    }
}

#[test]
fn invalid_column_comments_fail_with_field_context_before_rendering() {
    for comment in ["a\0b".to_owned(), "中".repeat(1025)] {
        let error = normalize_resource(spec(&comment), "order.toml", "bad")
            .unwrap_err()
            .to_string();
        assert!(error.contains("字段 name") && error.contains("未写入"));
    }
    assert!(normalize_resource(spec(&"中".repeat(1024)), "order.toml", "limit").is_ok());
}
