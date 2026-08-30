use std::collections::BTreeSet;

use super::ci::resource_gate::affected_package_args_for_target;

fn packages(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn every_workspace_package_has_one_resource_gate_disposition() {
    for package in [
        "ryframe",
        "ryframe-adapters",
        "ryframe-api",
        "ryframe-application",
        "ryframe-auth",
        "ryframe-config",
        "ryframe-db",
        "ryframe-generator",
        "ryframe-kernel",
        "ryframe-macro",
        "ryframe-tenant-db",
        "xtask",
    ] {
        assert!(
            affected_package_args_for_target(
                "test",
                &packages(&[package]),
                "target/ci/backend",
                4,
            )
            .is_ok(),
            "{package} 缺少精确编译面"
        );
    }

    let unknown = packages(&["ryframe-unknown"]);
    assert!(
        affected_package_args_for_target("test", &unknown, "target/ci/backend", 4)
            .unwrap_err()
            .to_string()
            .contains("缺少 crate 精确编译面")
    );
}

#[test]
fn resource_cargo_surface_selects_only_runtime_api_contracts() {
    let affected = packages(&[
        "ryframe",
        "ryframe-adapters",
        "ryframe-api",
        "ryframe-application",
        "ryframe-db",
        "ryframe-generator",
        "ryframe-tenant-db",
        "xtask",
    ]);
    let clippy =
        affected_package_args_for_target("clippy", &affected, "target/ci/backend", 8).unwrap();
    let test = affected_package_args_for_target("test", &affected, "target/ci/backend", 4).unwrap();
    let feature_value = clippy
        .windows(2)
        .find_map(|pair| (pair[0] == "--features").then_some(pair[1].as_str()))
        .unwrap();

    for feature in [
        "ryframe/bin-api",
        "ryframe-application/test-support",
        "ryframe-db/migration",
        "ryframe-db/repositories",
        "ryframe-tenant-db/migration",
        "ryframe-tenant-db/repositories",
        "xtask/resource",
    ] {
        assert!(feature_value.split(',').any(|actual| actual == feature));
    }
    for forbidden in [
        "ryframe-adapters/image-processing",
        "ryframe-adapters/otel",
        "ryframe-adapters/redis-api",
        "ryframe-adapters/spreadsheet",
        "runtime-swagger-ui",
        "bin-worker",
        "bin-migrate",
        "bin-reset",
        "schema-import",
        "--all-features",
        "--all-targets",
    ] {
        assert!(!clippy.iter().any(|argument| argument.contains(forbidden)));
        assert!(!test.iter().any(|argument| argument.contains(forbidden)));
    }
    for forbidden_test in ["redis_real_protocol", "otel_https_real", "s3_https_real"] {
        assert!(!clippy.contains(&forbidden_test.to_owned()));
        assert!(!test.contains(&forbidden_test.to_owned()));
    }

    for bin in [
        "ryframe",
        "export_openapi",
        "export_mysql_snapshot",
        "xtask",
    ] {
        assert!(clippy.windows(2).any(|pair| pair == ["--bin", bin]));
    }
    assert!(!test.contains(&"--bin".to_owned()));
    assert!(
        test.windows(2)
            .any(|pair| pair == ["--test", "mysql_real_protocol"])
    );
    assert!(test.windows(2).any(|pair| pair == ["--test", "internal"]));
    assert!(affected_package_args_for_target("build", &affected, "target/ci/backend", 4).is_err());
}
