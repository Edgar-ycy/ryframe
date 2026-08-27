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
            name: "device".into(),
            module: "system".into(),
            profile: ResourceProfile::FlatCrud,
            labels: LabelsSpec {
                zh_cn: "设备".into(),
                en: "Device".into(),
            },
        },
        storage: StorageSpec {
            kind: StorageKind::ControlRow,
            tenant_field: None,
            configuration_versioned: true,
        },
        api: ApiSpec {
            path: "/api/v1/system/devices".into(),
            operations: OperationSpec {
                create: "post_system_devices".into(),
                read: "get_system_devices_by_id".into(),
                list: "get_system_devices".into(),
                update: "put_system_devices_by_id".into(),
                delete: "delete_system_devices_by_id".into(),
            },
        },
        access: AccessSpec {
            capability: "system.device".into(),
            owner_field: None,
            permissions: PermissionSpec {
                create: "system:device:create".into(),
                read: "system:device:read".into(),
                list: "system:device:list".into(),
                update: "system:device:update".into(),
                delete: "system:device:delete".into(),
            },
        },
        menu: MenuSpec {
            key: "system.device".into(),
            parent: "system".into(),
            order: 80,
            icon: None,
            labels: LabelsSpec {
                zh_cn: "设备管理".into(),
                en: "Devices".into(),
            },
        },
        route: RouteSpec {
            key: "SystemDevice".into(),
            path: "/system/device".into(),
        },
        soft_delete: None,
        audit: None,
        extensions: ExtensionSpec::default(),
    }
}

fn table() -> TableInfo {
    TableInfo {
        table_name: "sys_device".into(),
        comment: Some("设备".into()),
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
            name: "fk_device_tenant".into(),
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
            .any(|note| note.contains("外键 fk_device_tenant"))
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
    ResourceSpec::parse(&first_toml, "catalog/resources/device.toml")
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

#[cfg(feature = "schema-import")]
#[test]
fn schema_import_feature_exposes_mysql_readers() {
    let _ = ryframe_generator::import::inspect_existing_table;
    let _ = ryframe_generator::import::list_existing_tables;
}
