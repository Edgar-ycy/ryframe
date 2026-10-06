use std::fs;

use ryframe_generator::business::{BusinessGenerateOptions, generate_business_package};

#[test]
fn business_crate_generation_is_local_read_only_by_default_and_repeatable() {
    let workspace = tempfile::Builder::new()
        .prefix("business-generation-")
        .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
        .expect("应创建隔离业务工作区");
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryframe-sdk");
    fs::create_dir_all(workspace.path().join("order-business/src/resources"))
        .expect("应创建业务源码目录");
    fs::write(
        workspace.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"order-business\"]\nresolver = \"3\"\n",
    )
    .expect("应写入工作区清单");
    write_business_manifest(workspace.path(), &sdk);
    fs::write(
        workspace.path().join("order-business/src/lib.rs"),
        r#"pub mod resources;

pub fn resource_descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {
    vec![<resources::Order as ryframe_sdk::ResourceModel>::descriptor()]
}
"#,
    )
    .expect("应写入业务入口");
    write_order_model(workspace.path());

    let crate_root = workspace.path().join("order-business/src/resources");
    let preview = generate_business_package(BusinessGenerateOptions {
        current_dir: &crate_root,
        package: "order-business",
        model: Some("Order"),
        write: false,
    })
    .expect("预览应成功");
    assert!(!preview.created.is_empty());
    assert!(
        !workspace
            .path()
            .join("order-business/src/generated")
            .exists()
    );

    let written = generate_business_package(BusinessGenerateOptions {
        current_dir: &crate_root,
        package: "order-business",
        model: None,
        write: true,
    })
    .expect("写入应成功");
    assert!(!written.created.is_empty());
    assert_generated_business(workspace.path());

    let repeat = generate_business_package(BusinessGenerateOptions {
        current_dir: &crate_root,
        package: "order-business",
        model: None,
        write: false,
    })
    .expect("重复预览应成功");
    assert!(repeat.created.is_empty());
    assert!(repeat.updated.is_empty());
    assert!(repeat.removed.is_empty());

    assert_migration_is_immutable(workspace.path(), &crate_root);
}

fn write_business_manifest(workspace: &std::path::Path, sdk: &std::path::Path) {
    fs::write(
        workspace.join("order-business/Cargo.toml"),
        format!(
            "[package]\nname = \"order-business\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[package.metadata.ryframe]\nkind = \"business\"\nmodule = \"order\"\n\n[features]\ndefault = []\ncatalog = []\n\n[dependencies]\nryframe-sdk = {{ path = {:?} }}\n",
            sdk.to_string_lossy()
        ),
    )
    .expect("应写入业务清单");
}

fn write_order_model(workspace: &std::path::Path) {
    fs::write(
        workspace.join("order-business/src/resources/mod.rs"),
        r#"#[derive(ryframe_sdk::ResourceModel)]
#[resource(name = "order", title = "订单", table = "biz_order", database = "tenant")]
pub struct Order {
    #[resource(primary_key)]
    pub tenant_id: String,
    #[resource(primary_key, generated)]
    pub id: i64,
    #[resource(unique, filter, sort)]
    pub code: String,
    #[resource(read_only)]
    pub created_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
    #[resource(read_only)]
    pub updated_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
}
"#,
    )
    .expect("应写入资源模型");
}

fn assert_generated_business(workspace: &std::path::Path) {
    assert!(
        workspace
            .join("order-business/src/generated/entities/order.rs")
            .is_file()
    );
    assert!(
        workspace
            .join("order-business/migrations/m_resource_initial_order.rs")
            .is_file()
    );
    assert!(
        fs::read_to_string(workspace.join("order-business/src/generated/mod.rs"))
            .expect("应读取聚合模块")
            .contains("x-ryframe-crud-resources")
    );
    assert!(
        !workspace
            .join("order-business/src/resources/order.rs")
            .exists()
    );
}

fn assert_migration_is_immutable(workspace: &std::path::Path, crate_root: &std::path::Path) {
    let model_path = workspace.join("order-business/src/resources/mod.rs");
    let changed = fs::read_to_string(&model_path)
        .expect("应读取资源模型")
        .replace(
            "    pub code: String,",
            "    pub code: String,\n    pub remark: String,",
        );
    fs::write(model_path, changed).expect("应更新资源模型");
    let error = generate_business_package(BusinessGenerateOptions {
        current_dir: crate_root,
        package: "order-business",
        model: None,
        write: false,
    })
    .expect_err("已生成的初始迁移不得随模型变化被覆盖");
    assert!(error.to_string().contains("禁止覆盖"), "实际错误：{error}");
}
