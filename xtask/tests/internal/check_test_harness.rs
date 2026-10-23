use std::{fs, path::Path};

use super::check::architecture::{test_layout::test_harness, workspace};

struct Fixture {
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let parent = super::workspace::root_dir().join(".local-tests/test-harness");
        fs::create_dir_all(&parent).unwrap();
        let fixture = Self {
            directory: tempfile::tempdir_in(parent).unwrap(),
        };
        fixture.write("xtask/src/main.rs", "mod origin; fn main() {}\n");
        fixture.write(
            "xtask/src/origin.rs",
            concat!(
                "fn private_origin() -> u8 { 7 }\n",
                "#[cfg(test)] mod tests {\n",
                "#[test] fn normalization() { assert_eq!(super::private_origin(), 7); }\n",
                "#[test] fn whitespace() {}\n#[test] fn invalid_origin() {}\n}\n",
            ),
        );
        fixture.write(
            "xtask/tests/internal.rs",
            concat!(
                "#[path = \"../src/origin.rs\"] mod origin;\n",
                "#[test] fn internal_only() {}\n",
            ),
        );
        fixture
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn write(&self, name: &str, contents: &str) {
        let path = self.root().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn errors(&self) -> Vec<String> {
        test_harness::validate_pair(self.root(), "xtask/src/main.rs", "xtask/tests/internal.rs")
    }
}

#[test]
fn shared_owner_keeps_private_origin_tests_and_allows_internal_only_scenarios() {
    let fixture = Fixture::new();
    assert!(fixture.errors().is_empty());
    fixture.write("xtask/tests/internal.rs", "#[test] fn internal_only() {}\n");
    let errors = fixture.errors();
    for name in ["normalization", "whitespace", "invalid_origin"] {
        assert!(
            errors.iter().any(|error| error.contains(name)),
            "{errors:?}"
        );
    }
    fixture.write(
        "xtask/tests/internal.rs",
        concat!(
            "#[path = \"../src/origin.rs\"] mod origin;\n",
            "#[path = \"../src/origin.rs\"] mod same_physical_tests;\n",
            "#[test] fn internal_only() {}\n",
        ),
    );
    assert!(fixture.errors().is_empty());
    fixture.write(
        "xtask/src/origin.rs",
        concat!(
            "macro_rules! generated { () => { #[test] fn macro_case() {} } }\n",
            "generated!();\n#[test_case(1)] fn parameterized(value: u8) {}\n",
        ),
    );
    assert!(fixture.errors().is_empty());
}

#[test]
fn a_new_main_test_cannot_disappear_into_a_disabled_binary_harness() {
    let fixture = Fixture::new();
    fixture.write(
        "xtask/src/main.rs",
        concat!(
            "mod origin; fn main() {}\n",
            "#[cfg(test)] mod tests { #[test] fn added_main_case() {} }\n",
        ),
    );
    let errors = fixture.errors();
    assert_eq!(errors.len(), 4, "{errors:?}");
    assert!(
        errors
            .iter()
            .all(|error| error.contains("added_main_case") && error.contains("main.rs:2"))
    );
}

#[test]
fn missing_or_ambiguous_module_paths_block_the_gate_instead_of_dropping_tests() {
    let fixture = Fixture::new();
    fixture.write(
        "xtask/tests/internal.rs",
        "#[path = \"../src/missing.rs\"] mod origin;\n",
    );
    let errors = fixture.errors();
    assert_eq!(errors.len(), 4);
    assert!(errors.iter().all(|error| error.contains("missing.rs")));
    fixture.write(
        "xtask/tests/internal.rs",
        "#[path = \"../src/origin.rs\"] mod origin;\n",
    );
    fixture.write("xtask/src/origin/mod.rs", "#[test] fn alternate() {}\n");
    let errors = fixture.errors();
    assert_eq!(errors.len(), 4);
    assert!(
        errors
            .iter()
            .all(|error| error.contains("文件缺失或有歧义"))
    );
}

#[test]
fn each_platform_and_resource_variant_must_reach_its_own_colocated_tests() {
    let fixture = Fixture::new();
    for (condition, expected_platform, expected_resource) in [
        (
            "all(windows, not(feature = \"resource\"))",
            "windows",
            false,
        ),
        (
            "all(target_os = \"linux\", feature = \"resource\")",
            "linux",
            true,
        ),
    ] {
        fixture.write(
            "xtask/src/main.rs",
            &format!(
                "mod origin; #[cfg({condition})] mod platform {{ #[test] fn exclusive() {{}} }}\n"
            ),
        );
        let errors = fixture.errors();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains(&format!(
            "[{expected_platform} resource={expected_resource}]"
        )));
        assert!(errors[0].contains("platform::exclusive"));
    }
    fixture.write(
        "xtask/src/main.rs",
        concat!(
            "mod origin;\n",
            "#[cfg_attr(windows, path = \"win.rs\")]\n",
            "#[cfg_attr(not(windows), path = \"linux.rs\")] mod selected;\n",
        ),
    );
    fixture.write(
        "xtask/src/win.rs",
        "#![cfg(windows)]\n#[test] fn windows_only() {}\n",
    );
    fixture.write(
        "xtask/src/linux.rs",
        "#![cfg(unix)]\n#[test] fn linux_only() {}\n",
    );
    fixture.write(
        "xtask/tests/internal.rs",
        concat!(
            "#[path = \"../src/origin.rs\"] mod origin;\n",
            "#[cfg_attr(windows, path = \"../src/win.rs\")]\n",
            "#[cfg_attr(not(windows), path = \"../src/linux.rs\")] mod selected;\n",
        ),
    );
    assert!(fixture.errors().is_empty());
}

#[test]
fn unknown_cfg_and_unproven_macro_test_generation_block_the_gate() {
    let fixture = Fixture::new();
    for condition in [
        "target_arch = \"aarch64\"",
        "feature = \"future\"",
        "any(test, future_platform)",
        "all(any(), future_platform)",
        "not(test, test)",
    ] {
        fixture.write(
            "xtask/src/main.rs",
            &format!("#[cfg({condition})] mod hidden {{ #[test] fn cannot_skip() {{}} }}\n"),
        );
        let errors = fixture.errors();
        assert_eq!(errors.len(), 4, "{condition}: {errors:?}");
        assert!(
            errors
                .iter()
                .all(|error| error.contains("未知") && error.contains("main.rs:1"))
        );
    }
    for source in [
        "macro_rules! generated { () => { #[test] fn hidden() {} } } generated!();",
        "#[test_case(1)] fn generated(value: u8) {}",
        "external::generate_tests!();",
    ] {
        fixture.write("xtask/src/main.rs", source);
        let errors = fixture.errors();
        assert!(errors.len() >= 4, "{errors:?}");
        assert!(
            errors.iter().all(
                |error| error.contains("无法证明的测试生成构造") && error.contains("main.rs:1")
            )
        );
    }
}

#[test]
fn inline_paths_parse_failures_cycles_and_repository_escape_fail_closed() {
    let fixture = Fixture::new();
    fixture.write(
        "xtask/src/main.rs",
        concat!(
            "#[path = \"nested\"] mod inline {\n",
            "#[path = \"cases.rs\"] mod cases; }\n",
        ),
    );
    fixture.write("xtask/src/nested/cases.rs", "#[test] fn inline_case() {}\n");
    fixture.write(
        "xtask/tests/internal.rs",
        "#[path = \"../src/nested/cases.rs\"] mod cases;\n",
    );
    assert!(fixture.errors().is_empty());
    fixture.write("xtask/src/nested/cases.rs", "#[test] fn broken( {}\n");
    let errors = fixture.errors();
    assert_eq!(errors.len(), 4);
    assert!(errors.iter().all(|error| error.contains("AST 含语法错误")));
    fixture.write(
        "xtask/src/nested/cases.rs",
        "#[path = \"cases.rs\"] mod cycle;\n",
    );
    let errors = fixture.errors();
    assert_eq!(errors.len(), 4);
    assert!(errors.iter().all(|error| error.contains("递归文件环")));
    fixture.write(
        "xtask/src/main.rs",
        "#[path = \"../../../../Cargo.toml\"] mod outside;\n",
    );
    let errors = fixture.errors();
    assert_eq!(errors.len(), 4);
    assert!(
        errors
            .iter()
            .all(|error| error.contains("仓库内的文件") || error.contains("无法解析模块路径"))
    );
}

fn packages(fixture: &Fixture, bin_test: bool, owner_test: bool) -> workspace::Packages {
    let package = serde_json::from_value(serde_json::json!({
        "name": "xtask", "id": "fixture", "manifest_path": fixture.root().join("xtask/Cargo.toml"),
        "targets": [
            {"name":"xtask", "kind":["bin"], "src_path":fixture.root().join("xtask/src/main.rs"), "test":bin_test},
            {"name":"internal", "kind":["test"], "src_path":fixture.root().join("xtask/tests/internal.rs"), "test":owner_test}
        ]
    })).unwrap();
    workspace::Packages::from([("xtask".into(), package)])
}

#[test]
fn shared_registration_checks_real_targets_and_does_not_weaken_empty_bin_policy() {
    let fixture = Fixture::new();
    let raw: toml::Table = toml::from_str(concat!(
        "shared_bin_test_harnesses = [{ target = \"xtask:xtask\", ",
        "source_entry = \"xtask/src/main.rs\", owner_test_entry = \"xtask/tests/internal.rs\" }]\n",
    ))
    .unwrap();
    let disabled = Default::default();
    let mut errors = Vec::new();
    let registered = test_harness::validate_registrations(
        fixture.root(),
        &raw,
        &packages(&fixture, false, true),
        &disabled,
        &mut errors,
    );
    assert_eq!(registered.into_iter().collect::<Vec<_>>(), ["xtask:xtask"]);
    assert!(errors.is_empty(), "{errors:?}");
    for (bin, owner) in [(true, true), (false, false)] {
        errors.clear();
        test_harness::validate_registrations(
            fixture.root(),
            &raw,
            &packages(&fixture, bin, owner),
            &disabled,
            &mut errors,
        );
        assert!(!errors.is_empty());
    }
    errors.clear();
    test_harness::validate_registrations(
        fixture.root(),
        &raw,
        &packages(&fixture, false, true),
        &std::collections::BTreeSet::from(["xtask:xtask".into()]),
        &mut errors,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.contains("不得同时登记为空 bin"))
    );
}
