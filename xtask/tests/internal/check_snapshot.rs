use std::path::Path;

use super::check::backend_snapshot_export_args;

#[test]
fn openapi_snapshot_reuses_the_backend_verify_target() {
    assert_eq!(
        backend_snapshot_export_args(
            "target",
            "ryframe-api",
            "export_openapi",
            Path::new("target/xtask/openapi.json"),
        ),
        [
            "run",
            "--locked",
            "--target-dir",
            "target",
            "-p",
            "ryframe-api",
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
