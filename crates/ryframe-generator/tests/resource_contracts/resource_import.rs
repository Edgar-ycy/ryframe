use ryframe_generator::{
    AccessSpec, ApiSpec, ExtensionSpec, LabelsSpec, MenuSpec, OperationSpec, PermissionSpec,
    ResourceIdentitySpec, ResourceProfile, ResourceSpec, RouteSpec, StorageKind, StorageSpec,
    WidgetSpec,
    import::{
        ColumnInfo, ForeignKeyInfo, IndexInfo, ResourceDraftMetadata, TableInfo,
        draft_resource_from_table,
    },
};

fn metadata() -> ResourceDraftMetadata {
    ResourceDraftMetadata {
        identity: ResourceIdentitySpec {
            name: "order".into(),
            module: "system".into(),
            profile: ResourceProfile::FlatCrud,
            labels: LabelsSpec {
                zh_cn: "设备".into(),
                en: "Order".into(),
            },
        },
        storage: StorageSpec {
            kind: StorageKind::ControlRow,
            tenant_field: None,
            configuration_versioned: true,
        },
        api: ApiSpec {
            path: "/api/v1/system/orders".into(),
            operations: OperationSpec {
                create: "post_system_orders".into(),
                read: "get_system_orders_by_id".into(),
                list: "get_system_orders".into(),
                update: "put_system_orders_by_id".into(),
                delete: "delete_system_orders_by_id".into(),
            },
        },
        access: AccessSpec {
            system_tenant_only: false,
            capability: Some("business.order".into()),
            owner_field: None,
            permissions: PermissionSpec {
                create: "system:order:create".into(),
                read: "system:order:read".into(),
                list: "system:order:list".into(),
                update: "system:order:update".into(),
                delete: "system:order:delete".into(),
            },
        },
        menu: MenuSpec {
            key: "business.order".into(),
            parent: "system".into(),
            order: 80,
            icon: None,
            labels: LabelsSpec {
                zh_cn: "设备管理".into(),
                en: "Orders".into(),
            },
        },
        route: RouteSpec {
            key: "business.order".into(),
            path: "/system/order".into(),
        },
        soft_delete: None,
        audit: None,
        extensions: ExtensionSpec::default(),
    }
}

fn table() -> TableInfo {
    TableInfo {
        table_name: "sys_order".into(),
        comment: Some("设备".into()),
        collation: None,
        columns: vec![
            column("id", "bigint", true, false, true, Some("设备编号")),
            column("tenant_id", "varchar", false, false, false, Some("租户")),
            column(
                "name",
                "varchar(100)",
                false,
                false,
                false,
                Some("设备名称"),
            ),
            column("created_at", "datetime", false, false, false, None),
            column("del_flag", "char", false, false, false, None),
        ],
        indexes: vec![
            IndexInfo {
                name: "uk_tenant_name".into(),
                unique: true,
                index_type: "BTREE".into(),
                columns: vec!["tenant_id".into(), "name".into()],
            },
            IndexInfo {
                name: "PRIMARY".into(),
                unique: true,
                index_type: "BTREE".into(),
                columns: vec!["id".into()],
            },
            IndexInfo {
                name: "idx_name".into(),
                unique: false,
                index_type: "BTREE".into(),
                columns: vec!["name".into()],
            },
        ],
        foreign_keys: vec![ForeignKeyInfo {
            name: "fk_order_tenant".into(),
            columns: vec!["tenant_id".into()],
            referenced_table: "sys_tenant".into(),
            referenced_columns: vec!["tenant_id".into()],
        }],
        foreign_key_dependencies: vec!["sys_tenant".into()],
        schema_canonical: "stable-schema".into(),
    }
}

fn column(
    name: &str,
    data_type: &str,
    primary: bool,
    nullable: bool,
    auto_increment: bool,
    comment: Option<&str>,
) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: data_type.into(),
        column_type: data_type.into(),
        collation: None,
        default: None,
        extra: String::new(),
        rust_type: String::new(),
        is_nullable: nullable,
        is_primary_key: primary,
        is_unique: primary,
        is_auto_increment: auto_increment,
        comment: comment.map(str::to_owned),
    }
}

#[test]
fn table_import_is_deterministic_and_keeps_business_semantics_pending() {
    let first = draft_resource_from_table(&table(), metadata()).expect("应生成资源草案");
    let second = draft_resource_from_table(&table(), metadata()).expect("再次生成应成功");
    let first_toml = first.to_toml().expect("草案应能序列化");
    let second_toml = second.to_toml().expect("草案应能稳定序列化");

    assert_eq!(first_toml, second_toml);
    assert_eq!(first.spec.database.primary_key, ["id"]);
    assert_eq!(
        first.spec.database.id_generation,
        ryframe_generator::IdGeneration::AutoIncrement
    );
    assert_eq!(
        first
            .spec
            .database
            .indexes
            .iter()
            .map(|index| index.name.as_str())
            .collect::<Vec<_>>(),
        ["idx_name", "uk_tenant_name"]
    );
    assert!(first.spec.database.soft_delete.is_none());
    assert!(first.spec.database.audit.is_none());
    assert!(!first.spec.database.bootstrap_migration);
    assert_eq!(
        first.pending_fields,
        ["id", "tenant_id", "name", "created_at", "del_flag"]
    );
    assert!(
        first
            .pending_notes
            .iter()
            .any(|note| note.contains("auto_increment"))
    );
    assert!(
        first
            .pending_notes
            .iter()
            .any(|note| note.contains("外键 fk_order_tenant"))
    );
    for field in &first.spec.fields {
        assert!(!field.usage.create);
        assert!(!field.usage.update);
        assert!(!field.usage.read);
        assert!(!field.usage.list);
        assert!(!field.usage.filter);
        assert!(!field.usage.sort);
        assert!(matches!(field.widget, WidgetSpec::Hidden));
    }
    assert!(first_toml.contains("待确认字段"));
    assert!(!first_toml.contains("soft_delete"));
    assert!(!first_toml.contains("audit"));
    ResourceSpec::parse(&first_toml, "catalog/resources/order.toml")
        .expect("TOML 草案应保持 ResourceSpec 结构有效");
}

#[test]
fn table_import_rejects_unknown_types_instead_of_guessing() {
    let mut table = table();
    table.columns[2].data_type = "geometry".into();
    let error = draft_resource_from_table(&table, metadata())
        .expect_err("未知类型必须显式失败")
        .to_string();
    assert!(error.contains("字段 name"));
    assert!(error.contains("geometry"));
    assert!(error.contains("人工选择"));
}

#[test]
fn mysql_mapping_keeps_ranges_and_qualified_model_types() {
    use ryframe_generator::import::mysql_rust_type;
    for (sql, rust) in [
        ("tinyint(1)", "i8"),
        ("smallint unsigned", "u16"),
        ("mediumint", "i32"),
        ("bigint unsigned", "u64"),
        ("bool", "bool"),
        ("decimal(20,6)", "ryframe_sdk::rust_decimal::Decimal"),
        (
            "datetime(6)",
            "ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>",
        ),
        ("json", "ryframe_sdk::serde_json::Value"),
        ("varbinary(64)", "Vec<u8>"),
    ] {
        assert_eq!(mysql_rust_type(sql, false).unwrap(), rust);
        assert_eq!(
            mysql_rust_type(sql, true).unwrap(),
            format!("Option<{rust}>")
        );
    }
    for unknown in [
        "geometry",
        "enum('a','b')",
        "set('x','y')",
        "uuid",
        "bytea",
        "",
    ] {
        let error = mysql_rust_type(unknown, false).unwrap_err();
        assert!(error.contains(unknown));
        assert!(error.contains("显式转换"));
    }
}

#[test]
fn imported_models_escape_comments_and_reject_unusable_keys_before_write() {
    use ryframe_generator::import::{mysql_rust_type, rust_model_source};
    let mut source = table();
    for column in &mut source.columns {
        column.rust_type = mysql_rust_type(&column.column_type, column.is_nullable).unwrap();
    }
    source.comment = Some("订单\"\\\n下一行".into());
    let model = rust_model_source(&source, "control").unwrap();
    syn::parse_file(&model).expect("数据库注释必须生成为合法 Rust 字符串");
    assert!(model.contains("primary_key, auto_increment"));
    assert!(
        model.contains(
            "index(name = \"uk_tenant_name\", fields = [\"tenant_id\", \"name\"], unique)"
        )
    );
    assert!(model.contains("index(name = \"idx_name\", fields = [\"name\"])"));
    assert!(!model.contains("index(name = \"PRIMARY\""));
    assert!(
        rust_model_source(&source, "tenant")
            .unwrap_err()
            .contains("tenant_id")
    );
    source.columns[1].is_primary_key = true;
    source
        .indexes
        .iter_mut()
        .find(|index| index.name == "PRIMARY")
        .unwrap()
        .columns = vec!["tenant_id".into(), "id".into()];
    assert!(rust_model_source(&source, "tenant").is_ok());
    assert!(
        rust_model_source(&source, "control")
            .unwrap_err()
            .contains("复合主键")
    );
    source.columns[2].name = "type".into();
    assert!(
        rust_model_source(&source, "tenant")
            .unwrap_err()
            .contains("字段映射")
    );
    for column in &mut source.columns {
        column.is_primary_key = false;
    }
    assert!(
        rust_model_source(&source, "control")
            .unwrap_err()
            .contains("没有主键")
    );
}

#[test]
fn imported_non_btree_indexes_fail_before_model_write() {
    let mut source = table();
    source.indexes[0].index_type = "FULLTEXT".into();
    let error = ryframe_generator::import::rust_model_source(&source, "control").unwrap_err();
    assert!(
        error.contains("uk_tenant_name") && error.contains("BTREE") && error.contains("未写入")
    );
}

#[test]
fn imported_primary_keys_keep_index_order_and_map_the_physical_id_column() {
    use ryframe_generator::import::{mysql_rust_type, rust_model_source};
    let mut source = table();
    for column in &mut source.columns {
        column.rust_type = mysql_rust_type(&column.column_type, column.is_nullable).unwrap();
    }
    source.columns[0].name = "order_id".into();
    source.columns[1].is_primary_key = true;
    let primary = source
        .indexes
        .iter_mut()
        .find(|index| index.name == "PRIMARY")
        .unwrap();
    primary.columns = vec!["tenant_id".into(), "order_id".into()];
    let model = rust_model_source(&source, "tenant").unwrap();
    assert!(model.contains("primary_key = [\"tenant_id\", \"id\"]"));
    assert!(model.contains("column = \"order_id\", primary_key, auto_increment"));
    assert!(model.find("pub id:").unwrap() < model.find("pub tenant_id:").unwrap());
    let draft = draft_resource_from_table(&source, metadata()).unwrap();
    assert_eq!(draft.spec.database.primary_key, ["tenant_id", "order_id"]);
    assert_eq!(draft.spec.fields[0].name, "order_id");
    source
        .indexes
        .iter_mut()
        .find(|index| index.name == "PRIMARY")
        .unwrap()
        .columns
        .reverse();
    assert!(
        rust_model_source(&source, "tenant")
            .unwrap_err()
            .contains("不自动重排")
    );
    source
        .indexes
        .iter_mut()
        .find(|index| index.name == "PRIMARY")
        .unwrap()
        .columns = vec!["tenant_id".into(), "missing".into()];
    assert!(
        rust_model_source(&source, "tenant")
            .unwrap_err()
            .contains("不一致")
    );
    source
        .indexes
        .iter_mut()
        .find(|index| index.name == "PRIMARY")
        .unwrap()
        .columns = vec!["tenant_id".into(), "order_id".into()];
    source.columns[2].name = "id".into();
    assert!(
        rust_model_source(&source, "tenant")
            .unwrap_err()
            .contains("映射为 id 时与已有列冲突")
    );
}

#[cfg(feature = "schema-import")]
#[test]
fn schema_import_feature_exposes_mysql_readers() {
    let _ = ryframe_generator::import::inspect_existing_table;
    let _ = ryframe_generator::import::list_existing_tables;
}
