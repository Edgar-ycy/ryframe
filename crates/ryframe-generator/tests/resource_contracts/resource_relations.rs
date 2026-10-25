use std::{fs, path::PathBuf};

use ryframe_generator::{
    ResourceIr, ResourceSpec, load_resource, normalize_resource, render_resources,
};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/order.toml")
}

fn post_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog/resources/post.toml")
}

fn related_order(local_field: &str, target_resource: &str) -> ResourceIr {
    let source = fs::read_to_string(fixture_path()).expect("应读取 Order fixture");
    let relation = format!(
        "\n[[relations]]\nname = \"parent\"\nkind = \"belongs_to\"\nlocal_field = \"{local_field}\"\ntarget_resource = \"{target_resource}\"\n"
    );
    let source = source.replacen("\n[api]\n", &format!("{relation}\n[api]\n"), 1);
    let spec =
        ResourceSpec::parse(&source, "tests/fixtures/order.toml").expect("关系清单结构应有效");
    normalize_resource(spec, "tests/fixtures/order.toml", "relation-fixture")
        .expect("关系清单应能归一化")
}

fn content<'a>(catalog: &'a ryframe_generator::GeneratedCatalog, suffix: &str) -> &'a str {
    &catalog
        .assets
        .iter()
        .find(|asset| asset.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("缺少生成资产 {suffix}"))
        .content
}

fn physical_resources() -> Vec<ResourceIr> {
    let make = |name: &str| {
        let mut spec =
            ResourceSpec::parse(include_str!("../fixtures/order.toml"), "physical.toml").unwrap();
        spec.resource.name = name.into();
        spec.database.table = format!("biz_{name}");
        for value in [
            &mut spec.api.path,
            &mut spec.api.operations.create,
            &mut spec.api.operations.read,
            &mut spec.api.operations.list,
            &mut spec.api.operations.update,
            &mut spec.api.operations.delete,
            &mut spec.access.permissions.create,
            &mut spec.access.permissions.read,
            &mut spec.access.permissions.list,
            &mut spec.access.permissions.update,
            &mut spec.access.permissions.delete,
            &mut spec.menu.key,
            &mut spec.route.key,
            &mut spec.route.path,
        ] {
            *value = value.replace("order", name);
        }
        for permission in [
            &mut spec.access.permissions.create,
            &mut spec.access.permissions.read,
            &mut spec.access.permissions.list,
            &mut spec.access.permissions.update,
            &mut spec.access.permissions.delete,
        ] {
            *permission = permission.replace('_', "-");
        }
        spec.access.capability = Some(format!("business.{name}"));
        spec.database.primary_key = vec!["tenant_id".into(), "id".into()];
        spec
    };
    let parent = make("z_parent");
    let mut child = make("a_child");
    child
        .database
        .foreign_keys
        .push(ryframe_generator::ForeignKeySpec {
            name: "fk_child_parent".into(),
            columns: vec!["tenant_id".into(), "id".into()],
            referenced_table: parent.database.table.clone(),
            referenced_columns: vec!["tenant_id".into(), "id".into()],
            update_rule: ryframe_generator::import::ForeignKeyAction::Cascade,
            delete_rule: ryframe_generator::import::ForeignKeyAction::Restrict,
            match_option: ryframe_generator::import::ForeignKeyMatch::None,
        });
    vec![child, parent]
        .into_iter()
        .map(|spec| normalize_resource(spec, "physical.toml", "physical").unwrap())
        .collect()
}

#[test]
fn physical_foreign_keys_preserve_tenant_boundary_and_parent_first_catalog() {
    let resources = physical_resources();
    let catalog = render_resources(&resources).unwrap();
    let migration = content(&catalog, "a_child/migration.rs");
    assert!(migration.contains("FOREIGN KEY (`tenant_id`, `id`) REFERENCES `biz_z_parent` (`tenant_id`, `id`) ON UPDATE CASCADE ON DELETE RESTRICT"));
    let tables = content(&catalog, "ryframe-tenant-db/src/generated/catalog.rs");
    assert!(
        tables.find("table: \"biz_z_parent\"").unwrap()
            < tables.find("table: \"biz_a_child\"").unwrap()
    );
    assert!(tables.contains("foreign_key_dependencies: &[\"biz_z_parent\"]"));
    assert!(tables.contains("FOREIGN KEY") && tables.contains("CASCADE"));
    assert!(!content(&catalog, "a_child/model.rs").contains("Detail"));
    for resource in &resources {
        assert!(resource.relations.is_empty());
    }
}

#[test]
fn physical_foreign_keys_reject_unresolved_cycles_scope_and_incompatible_columns() {
    for scenario in 0..9 {
        let mut resources = physical_resources();
        match scenario {
            0 => resources[0].foreign_keys[0].referenced_table = "outside_schema.parent".into(),
            1 => resources[0].foreign_keys[0].referenced_table = "unmanaged".into(),
            2 => resources[1].storage = ryframe_generator::StorageKind::ControlRow,
            3 => {
                resources[0].foreign_keys[0].columns = vec!["name".into(), "tenant_id".into()];
                resources[0].foreign_keys[0].referenced_columns =
                    vec!["tenant_id".into(), "name".into()];
                for resource in &mut resources {
                    resource
                        .fields
                        .iter_mut()
                        .find(|field| field.name == "name")
                        .unwrap()
                        .validation
                        .max_length = Some(64);
                }
                resources[0]
                    .indexes
                    .iter_mut()
                    .find(|index| index.unique)
                    .unwrap()
                    .fields
                    .reverse();
            }
            4 => resources[0].foreign_keys[0].referenced_columns.reverse(),
            5 => {
                resources[1]
                    .fields
                    .iter_mut()
                    .find(|field| field.name == "id")
                    .unwrap()
                    .sql_type = Some("INT".into())
            }
            6 => {
                let mut key = resources[0].foreign_keys[0].clone();
                key.name = "fk_parent_child".into();
                key.referenced_table = resources[0].table.clone();
                resources[1].foreign_keys.push(key);
            }
            7 => {
                let mut key = resources[0].foreign_keys[0].clone();
                key.referenced_table = resources[0].table.clone();
                resources[1].foreign_keys.push(key);
            }
            _ => {
                resources[0].foreign_keys[0].delete_rule =
                    ryframe_generator::import::ForeignKeyAction::SetNull
            }
        }
        let failure = render_resources(&resources).unwrap_err();
        if scenario == 3 {
            assert!(failure.to_string().contains("tenant_id"), "{failure}");
        }
    }
}

#[test]
fn belongs_to_relation_generates_one_read_slice_and_all_registrations() {
    let order = related_order("id", "order");
    let catalog = render_resources(&[order]).expect("同存储关系应能生成");

    let model = content(&catalog, "order/model.rs");
    assert!(model.contains("pub struct OrderDetail"));
    assert!(model.contains("pub parent: Option<crate::generated::order::OrderRecord>"));

    let port = content(&catalog, "order/port.rs");
    assert!(port.contains("AppResult<Option<OrderDetail>>"));
    assert!(port.contains("AppResult<PageResult<OrderRecord>>"));

    let repository = content(&catalog, "order/repository.rs");
    assert!(repository.contains("let Some(record) = entity::Entity::find_by_id"));
    assert!(repository.contains("Column::TenantId.eq(tenant_id)"));
    assert!(repository.contains("Column::DelFlag.eq(0_i32)"));
    assert!(repository.contains("OrderDetail { record, parent }"));
    assert!(repository.contains("fn to_parent_record"));

    let dto = content(&catalog, "order/dto.rs");
    assert!(dto.contains("pub struct OrderDetailVo"));
    assert!(dto.contains("#[serde(flatten)]"));
    assert!(dto.contains("pub parent: Option<crate::generated::order::dto::OrderVo>"));

    let handler = content(&catalog, "order/handler.rs");
    assert!(handler.contains("body = ApiResponse<OrderDetailVo>"));
    let openapi = content(&catalog, "order/openapi.rs");
    assert!(openapi.contains("super::dto::OrderDetailVo"));
    let app_registry = content(&catalog, "ryframe-application/src/generated/services.rs");
    assert!(app_registry.contains("pub order: Arc<OrderService>"));
    let api_registry = content(&catalog, "ryframe-api/src/generated/router.rs");
    assert!(api_registry.contains("super::order::handler::router"));
    assert!(api_registry.contains("CapabilityGuardState::new"));
    assert!(api_registry.contains("business.order"));
    let db_registry = content(&catalog, "ryframe-tenant-db/src/generated/mod.rs");
    assert!(db_registry.contains("ports.order = Some(order::port(router));"));

    let explanation = catalog.explanation("order").expect("应生成调用链说明");
    assert!(
        explanation
            .extension_notes
            .iter()
            .any(|note| note.contains("parent(id → order)"))
    );
}

#[test]
fn relation_rejects_unknown_targets_cross_storage_and_non_id_fields() {
    let unknown = related_order("id", "missing");
    let error = render_resources(&[unknown]).expect_err("未知目标必须失败");
    assert!(error.to_string().contains("未声明资源 `missing`"));

    let post = load_resource(post_path()).expect("Post 清单应有效");
    let cross_storage = related_order("id", "post");
    let error = render_resources(&[cross_storage, post]).expect_err("跨存储关系必须失败");
    assert!(
        error
            .to_string()
            .contains("跨越了 TenantData 与 ControlRow 存储")
    );

    let source = fs::read_to_string(fixture_path()).expect("应读取 Order fixture");
    let relation = "\n[[relations]]\nname = \"parent\"\nkind = \"belongs_to\"\nlocal_field = \"name\"\ntarget_resource = \"order\"\n";
    let source = source.replacen("\n[api]\n", &format!("{relation}\n[api]\n"), 1);
    let spec = ResourceSpec::parse(&source, "tests/fixtures/order.toml").unwrap();
    let error = normalize_resource(spec, "tests/fixtures/order.toml", "invalid")
        .expect_err("字符串外键必须失败");
    assert!(error.to_string().contains("belongs_to 关系字段必须是 i64"));
}

#[test]
fn relation_requires_matching_signedness_for_unsigned_target_ids() {
    let mut order = related_order("id", "order");
    order
        .fields
        .iter_mut()
        .find(|field| field.name == "id")
        .unwrap()
        .value_type = ryframe_generator::ValueType::U64;
    assert!(render_resources(&[order.clone()]).is_ok());
    order.relations[0].local_field = "status".into();
    let error = render_resources(&[order]).unwrap_err().to_string();
    assert!(error.contains("类型不一致"));
}
