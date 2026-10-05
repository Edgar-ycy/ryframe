use std::{collections::BTreeSet, path::Path};

use super::check::{
    BackendSnapshotProfile, backend_snapshot_export_args, prepare_backend_snapshots,
    prepare_consumer_backend_snapshots,
};

#[test]
fn snapshot_test_environment_is_stable_and_absolute() {
    let root = std::env::current_dir()
        .unwrap()
        .join(".local-tests")
        .join("stable-snapshot-environment");
    let profiles = BTreeSet::from([
        BackendSnapshotProfile::OpenApiContract,
        BackendSnapshotProfile::Mysql,
    ]);
    let first = prepare_backend_snapshots(&root, &profiles).unwrap();
    let second = prepare_backend_snapshots(&root, &profiles).unwrap();
    let consumer = prepare_consumer_backend_snapshots(&root, &profiles).unwrap();
    let first_environment = first.workspace_test_environment();
    assert_eq!(first_environment, second.workspace_test_environment());
    assert_ne!(first_environment, consumer.workspace_test_environment());
    assert_eq!(
        Path::new(&first_environment[0].1),
        root.join("target/xtask/verify-openapi.json")
    );
    assert_eq!(
        Path::new(&first_environment[1].1),
        root.join("target/xtask/verify-mysql.sql")
    );
    assert_eq!(
        Path::new(&consumer.workspace_test_environment()[0].1),
        root.join("target/xtask/consumer-openapi.json")
    );
    drop((first, second, consumer));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn combined_openapi_snapshot_reuses_the_backend_verify_target() {
    assert_eq!(
        backend_snapshot_export_args(
            "target",
            "ryframe",
            "export_openapi",
            Path::new("target/xtask/openapi.json"),
        ),
        [
            "--config",
            "profile.dev.debug=0",
            "run",
            "--locked",
            "--target-dir",
            "target",
            "-p",
            "ryframe",
            "--bin",
            "export_openapi",
            "--",
            "target/xtask/openapi.json",
        ]
    );
}

#[test]
fn mysql_snapshot_enables_the_required_migration_feature() {
    assert_eq!(
        backend_snapshot_export_args(
            "target",
            "ryframe-db",
            "export_mysql_snapshot",
            Path::new("target/xtask/mysql.sql"),
        ),
        [
            "--config",
            "profile.dev.debug=0",
            "run",
            "--locked",
            "--target-dir",
            "target",
            "-p",
            "ryframe-db",
            "--features",
            "migration",
            "--bin",
            "export_mysql_snapshot",
            "--",
            "target/xtask/mysql.sql",
        ]
    );
}
