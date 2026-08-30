use std::path::PathBuf;

use ryframe_generator::{load_resource, render_resources};

fn device() -> ryframe_generator::ResourceIr {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/device.toml");
    load_resource(path).expect("Device 资源清单应有效")
}

#[test]
fn aggregate_rust_modules_ignore_resource_source_hashes() {
    let original = device();
    let mut changed = original.clone();
    changed.source_hash = "source-hash-for-a-non-structural-change".to_owned();
    let before = render_resources(&[original]).expect("原始资源应能生成");
    let after = render_resources(&[changed]).expect("变更资源应能生成");
    for path in [
        "crates/ryframe-api/src/generated/crud_resources.rs",
        "crates/ryframe-api/src/generated/mod.rs",
        "crates/ryframe-api/src/generated/openapi.rs",
        "crates/ryframe-api/src/generated/router.rs",
        "crates/ryframe-application/src/generated/mod.rs",
        "crates/ryframe-application/src/generated/services.rs",
        "crates/ryframe-db/src/generated/mod.rs",
        "crates/ryframe-tenant-db/src/generated/mod.rs",
    ] {
        let before_content = before
            .assets
            .iter()
            .find(|asset| asset.path == path)
            .unwrap_or_else(|| panic!("原始生成缺少聚合模块 {path}"));
        let after_content = after
            .assets
            .iter()
            .find(|asset| asset.path == path)
            .unwrap_or_else(|| panic!("变更生成缺少聚合模块 {path}"));
        assert_eq!(
            before_content.content, after_content.content,
            "聚合模块 {path} 不应仅因源哈希变化"
        );
    }
}
