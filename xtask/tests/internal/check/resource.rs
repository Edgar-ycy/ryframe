use super::super::check::validate_resource_conflict_tests;
use super::{Path, resource_test_executable_from_messages};

#[test]
fn resource_test_executable_is_read_from_cargo_json_messages() {
    let output = concat!(
        "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"other\"},\"executable\":null}\n",
        "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"resource_workspace_compilation\",\"kind\":[\"test\"]},",
        "\"executable\":\"D:\\\\target\\\\resource-test.exe\"}\n"
    );
    assert_eq!(
        resource_test_executable_from_messages(output, "resource_workspace_compilation").unwrap(),
        Path::new("D:\\target\\resource-test.exe")
    );
    let second = serde_json::json!({"reason":"compiler-artifact", "target":{"name":"generation_pipeline", "kind":["test"]}, "executable":"D:/target/conflicts.exe"}).to_string();
    let combined = format!("{output}{second}\n");
    assert_eq!(
        resource_test_executable_from_messages(&combined, "generation_pipeline").unwrap(),
        Path::new("D:/target/conflicts.exe")
    );
    assert!(resource_test_executable_from_messages(&combined, "missing").is_err());
    assert!(
        resource_test_executable_from_messages(
            &format!("{combined}{second}\n"),
            "generation_pipeline"
        )
        .is_err()
    );
    assert!(
        resource_test_executable_from_messages(
            &second.replace("\"test\"", "\"bin\""),
            "generation_pipeline"
        )
        .is_err()
    );
}

#[test]
fn resource_conflict_test_listing_rejects_zero_partial_extra_or_duplicate_matches() {
    let structure = "cross_module_structure_conflicts_reject_before_install_with_or_without_frontend_sync: test\n";
    let permission = "cross_module_permission_conflict_rejects_before_install_with_or_without_frontend_sync: test\n";
    validate_resource_conflict_tests(&format!("{structure}{permission}\n2 tests, 0 benchmarks\n"))
        .unwrap();
    for listed in [
        String::new(),
        "0 tests, 0 benchmarks".into(),
        structure.into(),
        format!("{structure}{permission}cross_module_extra: test\n"),
        format!("{structure}{permission}{permission}"),
    ] {
        assert!(validate_resource_conflict_tests(&listed).is_err());
    }
}
