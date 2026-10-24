use std::collections::BTreeMap;

use serde_json::json;

use super::from_inputs;

const CARGO: &str = "  cargo 1.98.1 fixture\n";
const RUSTC: &str =
    "rustc 1.98.1 fixture\nbinary: rustc\nhost: x86_64-pc-windows-msvc\nrelease: 1.98.1\n";

#[test]
fn fixed_versions_and_host_fallback_keep_parent_commands_and_dev_defaults() {
    let result = from_inputs(CARGO, RUSTC, &BTreeMap::new()).unwrap();
    assert_eq!(result["profile"], "dev");
    assert_eq!(result["target"], "x86_64-pc-windows-msvc");
    assert_eq!(result["jobs"], "cargo-default");
    assert_eq!(
        result["toolchain"],
        json!({"cargo":CARGO.trim(),"rustc":RUSTC.trim()})
    );
    for role in ["api", "worker"] {
        assert_eq!(
            result["commands"][role],
            json!(super::super::build_command(role).unwrap())
        );
    }
}

#[test]
fn explicit_target_and_jobs_are_trimmed_and_empty_values_fall_back() {
    let environment = BTreeMap::from([
        (
            "CARGO_BUILD_TARGET".into(),
            "  aarch64-unknown-linux-gnu  ".into(),
        ),
        ("CARGO_BUILD_JOBS".into(), "  0004  ".into()),
    ]);
    let result = from_inputs(CARGO, "rustc fixture without host", &environment).unwrap();
    assert_eq!(result["target"], "aarch64-unknown-linux-gnu");
    assert_eq!(result["jobs"], "0004");
    let empty = BTreeMap::from([
        ("CARGO_BUILD_TARGET".into(), "  ".into()),
        ("CARGO_BUILD_JOBS".into(), "  ".into()),
    ]);
    let result = from_inputs(CARGO, RUSTC, &empty).unwrap();
    assert_eq!(result["target"], "x86_64-pc-windows-msvc");
    assert_eq!(result["jobs"], "cargo-default");
}

#[test]
fn allowed_tool_environment_is_sorted_hashed_and_never_exposes_values() {
    let mut environment = BTreeMap::from([
        ("RUSTFLAGS".into(), "secret-native-flags".into()),
        ("PATH".into(), "private-tools".into()),
        ("APP_ENV".into(), "test".into()),
        ("PASSWORD".into(), "do-not-record".into()),
    ]);
    let result = from_inputs(CARGO, RUSTC, &environment).unwrap();
    assert_eq!(
        result["environment"]["variables"],
        json!(["PATH", "RUSTFLAGS"])
    );
    assert_eq!(
        result["environment"]["sha256"],
        "c759df3f099efb0c5d78501fce533beb8c88193720e4a8d0e6c9c6606c8d31dc"
    );
    for secret in ["secret-native-flags", "private-tools", "do-not-record"] {
        assert!(!result.to_string().contains(secret));
    }
    environment.insert("PASSWORD".into(), "changed-ignored".into());
    assert_eq!(from_inputs(CARGO, RUSTC, &environment).unwrap(), result);
    environment.insert("RUSTFLAGS".into(), "changed-tool-input".into());
    assert_ne!(
        from_inputs(CARGO, RUSTC, &environment).unwrap()["environment"],
        result["environment"]
    );
}

#[test]
fn allowed_prefixes_and_platform_name_selection_keep_actual_names() {
    let environment = BTreeMap::from([
        ("AR_x86_64_pc_windows_msvc".into(), "archive-tool".into()),
        ("AWS_LC_SYS_CMAKE_BUILDER".into(), "1".into()),
        ("PKG_CONFIG_PATH".into(), "pkg-location".into()),
        ("rustflags".into(), "lowercase-flags".into()),
        ("APP_SECRET".into(), "ignored".into()),
    ]);
    let result = from_inputs(CARGO, RUSTC, &environment).unwrap();
    let mut expected = vec![
        "AR_x86_64_pc_windows_msvc",
        "AWS_LC_SYS_CMAKE_BUILDER",
        "PKG_CONFIG_PATH",
    ];
    if cfg!(windows) {
        expected.push("rustflags");
    }
    assert_eq!(result["environment"]["variables"], json!(expected));
}

#[test]
fn invalid_jobs_empty_versions_and_missing_target_fail_without_environment_values() {
    for jobs in ["0", "000", "-1", "1.5", "secret-invalid-jobs"] {
        let environment = BTreeMap::from([("CARGO_BUILD_JOBS".into(), jobs.into())]);
        let error = from_inputs(CARGO, RUSTC, &environment)
            .unwrap_err()
            .to_string();
        assert!(!error.contains(jobs));
    }
    assert!(from_inputs("  ", RUSTC, &BTreeMap::new()).is_err());
    assert!(from_inputs(CARGO, "\n", &BTreeMap::new()).is_err());
    assert!(from_inputs(CARGO, "rustc fixture without host", &BTreeMap::new()).is_err());
}

#[test]
fn version_diagnostics_explain_causes_and_remedies_without_private_values() {
    use super::{VersionFailure, version_diagnostic};
    use std::io::{Error, ErrorKind};
    let missing = Error::new(ErrorKind::NotFound, "private-path/secret-token");
    let denied = Error::new(ErrorKind::PermissionDenied, "private-path/secret-token");
    let os_error = Error::from_raw_os_error(2);
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(7)
    };
    #[cfg(unix)]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(7 << 8)
    };
    let cases = [
        (VersionFailure::Spawn(&missing), "ErrorKind=NotFound"),
        (VersionFailure::Spawn(&denied), "ErrorKind=PermissionDenied"),
        (VersionFailure::Spawn(&os_error), "OS错误码=Some(2)"),
        (VersionFailure::Exit(status), "code=Some(7)"),
        (VersionFailure::InvalidUtf8, "不是有效 UTF-8"),
        (VersionFailure::Empty, "为空或仅含空白"),
    ];
    for (failure, reason) in cases {
        let message = version_diagnostic("rustc", "-vV", failure);
        for expected in [
            "rustc -vV",
            reason,
            "修复办法",
            "PATH",
            "rust-toolchain.toml",
            "rustup show",
            "重装",
        ] {
            assert!(message.contains(expected));
        }
        assert!(!message.contains("private-path"));
        assert!(!message.contains("secret-token"));
    }
    assert!(version_diagnostic("cargo", "-V", VersionFailure::Empty).contains("cargo -V"));
    assert!(
        !version_diagnostic(
            "private-path/secret-token",
            "secret-token",
            VersionFailure::Empty
        )
        .contains("secret-token")
    );
}
