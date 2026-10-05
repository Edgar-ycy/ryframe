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
    let frontend = content(&catalog, "order/api.ts");
    assert!(frontend.contains("export type OrderDetail = ApiSchema<'OrderDetailVo'>"));

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
