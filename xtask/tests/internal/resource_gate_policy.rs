use super::*;

#[test]
fn parses_git_status_and_rejects_unknown_status() {
    let parsed = parse_name_status("M\0a.rs\0R100\0old.rs\0new.rs\0").unwrap();
    assert_eq!(parsed[0], changed(ChangeStatus::Modified, "a.rs"));
    assert_eq!(parsed[1].status, ChangeStatus::Renamed);
    assert_eq!(parsed[1].old_path.as_deref(), Some("old.rs"));
    assert!(parse_name_status("T\0a.rs\0").is_err());
}

#[test]
fn ownership_parser_is_strict_and_commands_share_ci_target() {
    let ownership = "format_version = 1\ngenerator_version = \"1.2.0\"\n\
        [[entries]]\nresource = \"post\"\nroot = \"backend\"\npath = \"crates/x.rs\"\n\
        source_hash = \"s\"\ncontent_hash = \"c\"\n";
    assert_eq!(parse_ownership(ownership).unwrap().entries.len(), 1);
    assert!(parse_ownership(&format!("{ownership}unknown = true\n")).is_err());

    let changed_source = ownership.replace("source_hash = \"s\"", "source_hash = \"new\"");
    assert_eq!(
        parse_ownership(ownership).unwrap().entries[0].fingerprint,
        parse_ownership(&changed_source).unwrap().entries[0].fingerprint,
        "来源清单哈希不应让字节未变的生成文件进入 Rust 编译面"
    );
    let changed_content = ownership.replace("content_hash = \"c\"", "content_hash = \"new\"");
    assert_ne!(
        parse_ownership(ownership).unwrap().entries[0].fingerprint,
        parse_ownership(&changed_content).unwrap().entries[0].fingerprint
    );

    let packages = set(&["ryframe", "ryframe-api"]);
    let clippy =
        affected_package_args_for_target("clippy", &packages, "target/ci/backend", 3).unwrap();
    assert!(
        clippy
            .windows(2)
            .any(|pair| pair == ["--target-dir", "target/ci/backend"])
    );
    assert!(clippy.contains(&"--no-default-features".to_owned()));
    assert!(!clippy.contains(&"--all-features".to_owned()));
    assert!(!clippy.contains(&"--all-targets".to_owned()));
    assert!(clippy.windows(2).any(|pair| pair == ["--lib", "--bin"]));
    assert!(
        clippy
            .windows(2)
            .any(|pair| pair == ["--bin", "export_openapi"])
    );
    assert!(
        clippy
            .windows(2)
            .any(|pair| pair == ["--test", "resource_api_contracts"])
    );
    let resource_args = resource_check_args_for_target(
        std::path::Path::new("../ryframe-vue3"),
        "target/ci/resource",
    );
    assert_eq!(
        resource_args,
        vec![
            "run".to_owned(),
            "--locked".to_owned(),
            "--target-dir".to_owned(),
            "target/ci/resource".to_owned(),
            "-p".to_owned(),
            "xtask".to_owned(),
            "--features".to_owned(),
            "resource".to_owned(),
            "--".to_owned(),
            "generate".to_owned(),
            "resource".to_owned(),
            "--all".to_owned(),
            "--check".to_owned(),
            "--frontend-dir".to_owned(),
            "../ryframe-vue3".to_owned(),
        ]
    );
}

#[test]
fn targeted_test_artifacts_are_exact_and_complete() {
    let packages = set(&["ryframe-api", "ryframe-application", "ryframe-db"]);
    let args = affected_package_args_for_target("test", &packages, "target/ci/backend", 8).unwrap();
    let expected = targeted_test_names_from_args(&args).unwrap();
    assert_eq!(
        expected,
        set(&[
            "resource_api_contracts",
            "resource_application_contracts",
            "mapping_contracts",
        ])
    );

    let output = expected
        .iter()
        .map(|name| {
            format!(
                r#"{{"reason":"compiler-artifact","target":{{"name":"{name}"}},"executable":"D:/target/{name}.exe"}}"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let executables = targeted_test_executables_from_messages(&output, &expected).unwrap();
    assert_eq!(
        executables.keys().cloned().collect::<BTreeSet<_>>(),
        expected
    );

    assert!(
        targeted_test_executables_from_messages(output.lines().next().unwrap(), &expected)
            .unwrap_err()
            .to_string()
            .contains("缺少定向测试产物")
    );
    let duplicated = format!("{output}\n{output}");
    assert!(
        targeted_test_executables_from_messages(&duplicated, &expected)
            .unwrap_err()
            .to_string()
            .contains("重复返回定向测试产物")
    );
}

#[test]
fn ci_plan_selects_resource_and_full_fallback_surfaces_only() {
    for path in [
        "catalog/resources/post.toml",
        "catalog/resources/.ownership.toml",
        "crates/ryframe-api/src/generated/post/mod.rs",
        "crates/ryframe-generator/src/resource/render.rs",
        "Cargo.lock",
        ".github/workflows/ci.yml",
        "architecture/crate-boundaries.toml",
        "openapi/openapi.json",
    ] {
        assert!(should_run_for_paths(&[path.to_owned()]), "{path}");
    }
    for path in [
        "README.md",
        "docs/development.md",
        "crates/ryframe-api/src/handlers/auth.rs",
        "config/default.toml",
    ] {
        assert!(!should_run_for_paths(&[path.to_owned()]), "{path}");
    }
}

#[test]
fn configured_ci_ref_prefers_explicit_range_and_ignores_empty_values() {
    assert_eq!(
        preferred_nonempty_ref(Some("head"), Some("github")),
        Some("head".to_owned())
    );
    assert_eq!(
        preferred_nonempty_ref(Some("  "), Some("github")),
        Some("github".to_owned())
    );
    assert_eq!(preferred_nonempty_ref(None, Some("")), None);
}
