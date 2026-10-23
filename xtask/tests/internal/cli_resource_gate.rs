use super::*;
use crate::cli::ResourceGateOptions;

#[test]
fn parses_complete_resource_gate_options_without_environment() {
    let path = std::env::temp_dir().join("资源 gate decision.json");
    let sha = "ABCDEF1234567890ABCDEF1234567890ABCDEF12";
    let parsed = parse_command(&[
        "check",
        "ci",
        "resource-gate",
        "--base-sha",
        sha,
        "--head-sha",
        sha,
        "--frontend-ref",
        sha,
        "--targeted",
        "--decision-file",
        path.to_str().unwrap(),
        "--jobs",
        "1",
        "--test-jobs",
        "64",
    ])
    .unwrap();
    assert_eq!(
        parsed,
        Command::Check(CheckCommand::Ci(CiCommand::ResourceGate(
            ResourceGateOptions {
                base_sha: Some(sha.to_ascii_lowercase()),
                head_sha: Some(sha.to_ascii_lowercase()),
                frontend_ref: Some(sha.to_ascii_lowercase()),
                targeted: true,
                decision_file: Some(path),
                jobs: Some(1),
                test_jobs: Some(64),
            }
        )))
    );
}

#[test]
fn rejects_invalid_or_duplicate_resource_gate_options() {
    for flag in ["--base-sha", "--head-sha", "--frontend-ref"] {
        for invalid in [
            "",
            "HEAD",
            "123",
            "0000000000000000000000000000000000000000",
            "gggggggggggggggggggggggggggggggggggggggg",
        ] {
            assert!(
                parse_command(&["check", "ci", "resource-gate", flag, invalid]).is_err(),
                "{flag}"
            );
        }
    }
    for flag in ["--jobs", "--test-jobs"] {
        for invalid in ["0", "65", "-1", "1.5", "abc", ""] {
            assert!(parse_command(&["check", "ci", "resource-gate", flag, invalid]).is_err());
        }
    }
    let absolute = std::env::temp_dir().join("decision.json");
    let sha = "1".repeat(40);
    for (name, value) in [
        ("--base-sha", sha.as_str()),
        ("--head-sha", sha.as_str()),
        ("--frontend-ref", sha.as_str()),
        ("--decision-file", absolute.to_str().unwrap()),
        ("--jobs", "4"),
        ("--test-jobs", "4"),
    ] {
        assert!(parse_command(&["check", "ci", "resource-gate", name]).is_err());
        assert!(
            parse_command(&["check", "ci", "resource-gate", name, value, name, value]).is_err()
        );
    }
    for args in [
        vec!["--targeted", "--targeted"],
        vec!["--targeted", "true"],
        vec!["--decision-file", "relative.json"],
        vec!["--unknown", "value"],
    ] {
        let mut command = vec!["check", "ci", "resource-gate"];
        command.extend(args);
        assert!(parse_command(&command).is_err());
    }
}

#[test]
fn old_project_environment_protocol_is_absent_from_gate_sources() {
    let sources = [
        include_str!("../../src/ci/resource_gate.rs"),
        include_str!("../../src/ci/resource_gate/repository.rs"),
        include_str!("../../src/ci/resource_gate/replay/environment.rs"),
        include_str!("../../src/ci/resource_gate/replay/runner.rs"),
        include_str!("../../src/devex/metadata.rs"),
        include_str!("../../src/check/context.rs"),
        include_str!("../../src/ci.rs"),
    ];
    for source in sources {
        for name in [
            "RYFRAME_CI_HEAD_SHA",
            "RYFRAME_CI_FRONTEND_REF",
            "RYFRAME_RESOURCE_GATE_TARGETED",
            "RYFRAME_RESOURCE_GATE_DECISION_FILE",
            "RYFRAME_RESOURCE_GATE_TEST_JOBS",
            "RYFRAME_VERIFY_JOBS",
        ] {
            assert!(!source.contains(name), "残留项目环境协议：{name}");
        }
    }
}
