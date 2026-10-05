use std::{fs, path::Path, process::Command};

use ryframe_generator::{
    PlanAction, ResourceSpec, ResourceWorkspace, normalize_resource, plan_all_resource_changes,
    plan_resource_changes, render_resources, write_resource,
};

fn git(root: &Path, args: &[&str]) {
    let result = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn development_schema_changes_require_explicit_write_and_reach_zero_diff() {
    let backend = tempfile::tempdir().unwrap();
    let frontend = tempfile::tempdir().unwrap();
    fs::write(
        backend.path().join("Cargo.toml"),
        "[workspace.package]\nversion = \"0.12.1\"\n",
    )
    .unwrap();
    git(backend.path(), &["init"]);
    git(backend.path(), &["add", "Cargo.toml"]);
    git(
        backend.path(),
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "test",
        ],
    );
    let workspace = ResourceWorkspace {
        backend_root: backend.path(),
        frontend_root: Some(frontend.path()),
    };
    let source = include_str!("../fixtures/order.toml");
    let render = |source: &str| {
        let spec = ResourceSpec::parse(source, "catalog/resources/order.toml").unwrap();
        render_resources(&[
            normalize_resource(spec, "catalog/resources/order.toml", "fixture").unwrap(),
        ])
        .unwrap()
    };
    let original = render(source);
    write_resource(&original, "order", workspace).unwrap();
    let migration = backend
        .path()
        .join("crates/ryframe-tenant-db/src/generated/order/migration.rs");
    let ownership = backend.path().join("catalog/resources/.ownership.toml");
    let before = fs::read(&migration).unwrap();
    let ownership_before = fs::read(&ownership).unwrap();
    let changed = render(&source.replacen("max_length = 100", "max_length = 110", 1));
    for plan in [
        plan_resource_changes(&changed, "order", workspace).unwrap(),
        plan_all_resource_changes(&changed, workspace).unwrap(),
    ] {
        assert!(
            plan.assets
                .iter()
                .any(|asset| asset.path.ends_with("order/migration.rs")
                    && asset.action == PlanAction::Update)
        );
        assert_eq!(fs::read(&migration).unwrap(), before);
        assert_eq!(fs::read(&ownership).unwrap(), ownership_before);
    }
    write_resource(&changed, "order", workspace).unwrap();
    assert_ne!(fs::read(&migration).unwrap(), before);
    let repeated = write_resource(&changed, "order", workspace).unwrap();
    assert!(repeated.written.is_empty());
    assert!(repeated.removed.is_empty());
    assert!(
        plan_all_resource_changes(&changed, workspace)
            .unwrap()
            .assets
            .iter()
            .all(|asset| asset.action == PlanAction::Unchanged)
    );
}
