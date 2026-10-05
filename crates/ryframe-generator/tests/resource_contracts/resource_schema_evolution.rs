use ryframe_generator::{
    GeneratedCatalog, ResourceSpec, ResourceWorkspace, load_resource, normalize_resource,
    render_resources, write_resource,
};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, process::Command};
fn order_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/order.toml")
}
fn order() -> ryframe_generator::ResourceIr {
    load_resource(order_path()).expect("Order 资源清单应有效")
}

fn assert_tenant_bootstrap_migration_gates(generated: &GeneratedCatalog) {
    let aggregate = generated
        .assets
        .iter()
        .find(|asset| asset.path == "crates/ryframe-tenant-db/src/generated/mod.rs")
        .expect("tenant generated 聚合应存在");
    assert!(
        aggregate
            .content
            .contains("Box::new(order::migration::Migration)")
    );
    assert!(
        aggregate
            .content
            .contains("pub const MIGRATION_NAMES: &[&str] = &[\"m_resource_initial_order\"];")
    );
    assert!(aggregate.content.contains(
        "#[cfg(any(feature = \"repositories\", feature = \"migration\"))]\npub mod order;"
    ));
    let database_slice = generated
        .assets
        .iter()
        .find(|asset| asset.path == "crates/ryframe-tenant-db/src/generated/order/mod.rs")
        .expect("应生成 tenant 数据库模块");
    assert!(
        database_slice
            .content
            .contains("#[cfg(feature = \"migration\")]\npub mod migration;")
    );
}

#[test]
fn initial_migration_is_immutable_and_schema_evolution_requires_new_revision() {
    let backend = tempfile::tempdir().expect("应创建后端临时工作区");
    let frontend = tempfile::tempdir().expect("应创建前端临时工作区");
    fs::write(
        backend.path().join("Cargo.toml"),
        "[workspace.package]\nversion = \"1.0.0\"\n",
    )
    .expect("应创建工作区标识");
    let workspace = ResourceWorkspace {
        backend_root: backend.path(),
        frontend_root: Some(frontend.path()),
    };
    let original = order();
    let rendered = render_resources(std::slice::from_ref(&original)).expect("Order 应生成");
    assert_tenant_bootstrap_migration_gates(&rendered);
    write_resource(&rendered, "order", workspace).expect("首次写入应成功");
    let migration_path = backend
        .path()
        .join("crates/ryframe-tenant-db/src/generated/order/migration.rs");
    let initial_migration = fs::read_to_string(&migration_path).expect("初始迁移应存在");

    let mut label_only = original;
    label_only.source_hash = "a".repeat(64);
    label_only.labels.zh_cn = "终端设备".into();
    write_resource(
        &render_resources(&[label_only]).expect("标签变更应生成"),
        "order",
        workspace,
    )
    .expect("非 schema 变更应保留初始迁移");
    assert_eq!(
        fs::read_to_string(&migration_path).unwrap(),
        initial_migration
    );

    let unversioned_source = fs::read_to_string(order_path())
        .expect("应读取 Order fixture")
        .replacen("max_length = 100", "max_length = 110", 1);
    let unversioned = normalize_resource(
        ResourceSpec::parse(&unversioned_source, "catalog/resources/order.toml").unwrap(),
        "catalog/resources/order.toml",
        "schema-without-revision",
    )
    .expect("缺少 revision 不影响清单语法校验");
    let error = write_resource(
        &render_resources(&[unversioned]).expect("未声明 revision 的 schema 仍应可预览"),
        "order",
        workspace,
    )
    .expect_err("已受管 schema 变化必须声明新 revision")
    .to_string();
    assert!(error.contains("没有声明新的 schema_revision"));
    assert!(error.contains("cargo xtask data migrate new tenant-data"));

    let revision = "m20260823_123456_expand_order_name";
    let changed_source = fs::read_to_string(order_path())
        .expect("应读取 Order fixture")
        .replacen(
            "bootstrap_migration = true",
            &format!("bootstrap_migration = true\nschema_revision = {revision:?}"),
            1,
        )
        .replacen("max_length = 100", "max_length = 120", 1);
    let spec = ResourceSpec::parse(&changed_source, "catalog/resources/order.toml")
        .expect("schema 变更 TOML 应有效");
    let changed = normalize_resource(spec, "catalog/resources/order.toml", "schema-v2")
        .expect("schema v2 应通过 IR");
    let revision_path = backend
        .path()
        .join("crates/ryframe-tenant-db/src/migration")
        .join(format!("{revision}.rs"));
    fs::create_dir_all(revision_path.parent().unwrap()).expect("应创建追加迁移目录");
    fs::write(&revision_path, "// 待冻结的追加 roll-forward 迁移\n").expect("应写入追加迁移");
    write_resource(
        &render_resources(&[changed]).expect("schema v2 应生成"),
        "order",
        workspace,
    )
    .expect("新 revision 与未提交迁移应允许 schema 演进");
    assert_eq!(
        fs::read_to_string(&migration_path).unwrap(),
        initial_migration
    );
    let ownership = fs::read_to_string(backend.path().join("catalog/resources/.ownership.toml"))
        .expect("ownership 应存在");
    assert!(ownership.contains("schema_hash ="));
    assert!(ownership.contains(&format!("schema_revision = {revision:?}")));

    assert_invalid_revision_updates(workspace, &changed_source);
}

fn assert_invalid_revision_updates(workspace: ResourceWorkspace<'_>, changed_source: &str) {
    let reused_source = changed_source.replacen("max_length = 120", "max_length = 130", 1);
    let reused = normalize_resource(
        ResourceSpec::parse(&reused_source, "catalog/resources/order.toml").unwrap(),
        "catalog/resources/order.toml",
        "schema-v3",
    )
    .expect("schema v3 IR 本身应有效");
    let error = write_resource(
        &render_resources(&[reused]).expect("schema v3 应生成预览"),
        "order",
        workspace,
    )
    .expect_err("同 revision 不得承载第二次 schema 变化")
    .to_string();
    assert!(error.contains("已用于上一版 schema"));
    assert!(error.contains("cargo xtask data migrate new tenant-data"));

    let removed_source = changed_source.replacen(
        "bootstrap_migration = true",
        "bootstrap_migration = false",
        1,
    );
    let removed = normalize_resource(
        ResourceSpec::parse(&removed_source, "catalog/resources/order.toml").unwrap(),
        "catalog/resources/order.toml",
        "schema-v2-without-initial",
    )
    .expect("移除初始迁移标记仍是有效 IR");
    let error = write_resource(
        &render_resources(&[removed]).expect("无初始迁移资产的预览应可构造"),
        "order",
        workspace,
    )
    .expect_err("已落盘初始迁移不得被清单删除")
    .to_string();
    assert!(error.contains("初始迁移不可删除或改名"));
}

#[test]
fn frozen_migration_lock_is_verified_before_any_generated_write() {
    let backend = tempfile::tempdir().expect("应创建后端临时工作区");
    let frontend = tempfile::tempdir().expect("应创建前端临时工作区");
    fs::write(
        backend.path().join("Cargo.toml"),
        "[workspace.package]\nversion = \"0.12.1\"\n",
    )
    .unwrap();
    init_git(backend.path());
    let workspace = ResourceWorkspace {
        backend_root: backend.path(),
        frontend_root: Some(frontend.path()),
    };
    let catalog = render_resources(&[order()]).unwrap();
    write_resource(&catalog, "order", workspace).unwrap();
    let relative = "crates/ryframe-tenant-db/src/migration/m20260823_123456_order_state.rs";
    let migration = backend.path().join(relative);
    fs::create_dir_all(migration.parent().unwrap()).unwrap();
    fs::write(&migration, "// frozen\n").unwrap();
    let hash = hex::encode(Sha256::digest(fs::read(&migration).unwrap()));
    write_lock(
        backend.path(),
        relative,
        &hash,
        "tenant-data",
        "m20260823_123456_order_state",
    );
    assert!(
        write_resource(&catalog, "order", workspace)
            .unwrap()
            .written
            .is_empty()
    );

    fs::write(&migration, "// tampered\n").unwrap();
    assert_lock_rejected(&catalog, workspace, "冻结迁移被修改");
    fs::write(&migration, "// frozen\n").unwrap();

    write_lock(
        backend.path(),
        "crates/ryframe-tenant-db/src/migration/m20260823_123456_missing.rs",
        &hash,
        "tenant-data",
        "m20260823_123456_missing",
    );
    assert_lock_rejected(&catalog, workspace, "冻结迁移被删除、改名或无法读取");
    write_lock(
        backend.path(),
        relative,
        &hash,
        "control",
        "m20260823_123456_order_state",
    );
    assert_lock_rejected(&catalog, workspace, "身份无效");
    write_lock(
        backend.path(),
        relative,
        &hash,
        "tenant-data",
        "m20260823_123456_other",
    );
    assert_lock_rejected(&catalog, workspace, "身份无效");
    write_lock(
        backend.path(),
        "../escape.rs",
        &hash,
        "tenant-data",
        "escape",
    );
    assert_lock_rejected(&catalog, workspace, "路径不安全");
}

fn init_git(root: &std::path::Path) {
    for arguments in [
        vec!["init"],
        vec!["add", "Cargo.toml"],
        vec![
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "test",
        ],
    ] {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn write_lock(root: &std::path::Path, path: &str, hash: &str, storage: &str, target: &str) {
    let lock = root.join("catalog/migrations.lock.toml");
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(
        lock,
        format!(
            "format_version = 1\nfrozen_at = \"2026-08-23\"\n\n[[files]]\npath = {path:?}\nsha256 = {hash:?}\nstorage = {storage:?}\ntarget = {target:?}\n"
        ),
    )
    .unwrap();
}

fn assert_lock_rejected(
    catalog: &GeneratedCatalog,
    workspace: ResourceWorkspace<'_>,
    expected: &str,
) {
    let ownership = workspace
        .backend_root
        .join("catalog/resources/.ownership.toml");
    let generated = workspace
        .backend_root
        .join("crates/ryframe-tenant-db/src/generated/order/mod.rs");
    let ownership_before = fs::read(&ownership).unwrap();
    let generated_before = fs::read(&generated).unwrap();
    let error = write_resource(catalog, "order", workspace)
        .expect_err("无效冻结锁必须阻止生成")
        .to_string();
    assert!(error.contains(expected), "意外错误：{error}");
    assert_eq!(fs::read(ownership).unwrap(), ownership_before);
    assert_eq!(fs::read(generated).unwrap(), generated_before);
}
