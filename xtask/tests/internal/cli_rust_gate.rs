use super::parse_command;
use crate::cli::{CheckCommand, CiCommand, Command, RustGateOptions};

#[test]
fn parses_rust_gate_options_and_rejects_invalid_arguments() {
    assert_eq!(
        parse_command(&[
            "check",
            "ci",
            "rust-gate",
            "--frontend-ref",
            "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC",
            "--profile",
            "windows-smoke",
            "--test-jobs",
            "4",
        ])
        .unwrap(),
        Command::Check(CheckCommand::Ci(CiCommand::RustGate(RustGateOptions {
            frontend_ref: Some("cccccccccccccccccccccccccccccccccccccccc".to_owned()),
            profile: Some("windows-smoke".to_owned()),
            test_jobs: Some(4),
            ..Default::default()
        })))
    );
    for arguments in [
        ["--test-jobs", "0"].as_slice(),
        ["--jobs", "1"].as_slice(),
        ["--jobs", "0"].as_slice(),
        ["--jobs", "65"].as_slice(),
        ["--target-dir", "relative-target"].as_slice(),
        ["--jobs", "6", "--jobs", "4"].as_slice(),
        ["--plan", "--plan"].as_slice(),
        ["--profile", "windows-smoke", "--plan"].as_slice(),
    ] {
        let command = [&["check", "ci", "rust-gate"], arguments].concat();
        assert!(parse_command(&command).is_err(), "{arguments:?}");
    }
}
