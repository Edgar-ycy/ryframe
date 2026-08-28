use std::{fs, path::PathBuf};

use ryframe_generator::{ResourceSpec, normalize_resource, render_resources};

fn post_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog/resources/post.toml")
}

#[test]
fn control_initial_migration_has_tenant_fk_and_feature_gates() {
    let source = fs::read_to_string(post_path())
        .expect("应读取 Post 清单")
        .replacen(
            "primary_key = [\"id\"]",
            "primary_key = [\"id\"]\nbootstrap_migration = true",
            1,
        );
    let spec = ResourceSpec::parse(&source, "catalog/resources/post.toml")
        .expect("control fixture TOML 应有效");
    let post = normalize_resource(spec, "catalog/resources/post.toml", "control-schema")
        .expect("control fixture 应通过 IR");
    let generated = render_resources(&[post]).expect("control fixture 应生成");
    let migration = generated
        .assets
        .iter()
        .find(|asset| asset.path.ends_with("post/migration.rs"))
        .expect("应生成 control 初始迁移")
        .content
        .as_str();

    for contract in [
        "CONSTRAINT `fk_post_tenant`",
        "REFERENCES `sys_tenant` (`tenant_id`)",
        "`tenant_id` VARCHAR(64)",
        "`status` VARCHAR(1)",
    ] {
        assert!(migration.contains(contract));
    }
    let aggregate = generated
        .assets
        .iter()
        .find(|asset| asset.path == "crates/ryframe-db/src/generated/mod.rs")
        .expect("control generated 聚合应存在");
    for contract in [
        "Box::new(post::migration::Migration)",
        "#[cfg(any(feature = \"repositories\", feature = \"migration\"))]\npub mod post;",
        "pub const MIGRATION_NAMES: &[&str] = &[\"m_resource_initial_post\"];",
        "#[cfg(feature = \"migration\")]\nuse sea_orm_migration::MigrationTrait;",
        "#[cfg(feature = \"migration\")]\npub fn migrations()",
    ] {
        assert!(aggregate.content.contains(contract));
    }
    let database_slice = generated
        .assets
        .iter()
        .find(|asset| asset.path == "crates/ryframe-db/src/generated/post/mod.rs")
        .expect("应生成 control 数据库模块");
    assert!(
        database_slice
            .content
            .contains("#[cfg(feature = \"migration\")]\npub mod migration;")
    );
}
