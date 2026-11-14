use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

use crate::{
    recovery::monitoring::permissions,
    windows_acl::{self, native},
};

fn icacls(path: &Path, arguments: &[String]) {
    let mut command = Command::new(native::system_directory().unwrap().join("icacls.exe"));
    command
        .arg(path)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output =
        crate::process::command_output_result_with_timeout(command, Duration::from_secs(10))
            .unwrap();
    assert!(output.status.success(), "隔离 ACL 变更失败");
}

#[test]
fn whoami_accepts_one_numeric_sid_without_decoding_or_echoing_account_names() {
    for bytes in [
        b"\"domain\\user\",\"S-1-5-21-1001\"\r\n".as_slice(),
        b"\"\xff\xfe name \"\"quoted\"\"\",\"S-1-5-21-1001\"\n".as_slice(),
    ] {
        assert_eq!(native::parse_whoami(bytes).unwrap(), "S-1-5-21-1001");
    }
    for bytes in [
        b"\"user\",\"S-1-5-1\"\n\"other\",\"S-1-5-2\"".as_slice(),
        b"\"user\",\"S-1-5-01\"".as_slice(),
        b"\"user\",\"S-1-5-4294967296\"".as_slice(),
        b"\"user\",\"S-1-281474976710656-1\"".as_slice(),
        b"\"private-secret\",\"not-a-sid\"".as_slice(),
        b"\"private-secret\",\"S-1-5-1\",\"extra\"".as_slice(),
    ] {
        let error = native::parse_whoami(bytes).unwrap_err();
        assert!(!error.contains("private-secret"));
    }
}

#[test]
fn real_private_directory_file_and_monitoring_acl_preserve_distinct_policies() {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("ACL 中文 空格 ' 单引号-")
        .tempdir_in(local)
        .unwrap()
        .keep();
    println!("ACL 测试隔离目录：{}", directory.display());
    windows_acl::secure_directory(&directory).unwrap();
    let initial = windows_acl::read(&directory).unwrap();
    initial.verify_mysql(true).unwrap();
    let file = directory.join("metrics-token.txt");
    fs::write(&file, b"private-test-only").unwrap();
    let inherited = windows_acl::read(&file).unwrap();
    inherited.verify_mysql(false).unwrap();
    assert!(permissions::validate_windows_acl(&inherited.monitoring_value()).is_err());
    assert!(windows_acl::secure_directory(&directory).is_err());
    assert_eq!(fs::read(&file).unwrap(), b"private-test-only");
    let current = initial.current_sid;
    icacls(
        &file,
        &[
            "/inheritance:r".into(),
            "/grant:r".into(),
            format!("*{current}:(RC,RD)"),
        ],
    );
    let missing_synchronize = permissions::security_receipt(&file)
        .unwrap_err()
        .to_string();
    assert!(missing_synchronize.contains("SYNCHRONIZE"));
    icacls(&file, &["/grant:r".into(), format!("*{current}:(RC,RD,S)")]);
    let read_only = windows_acl::read(&file).unwrap();
    assert_eq!(read_only.entries.len(), 1);
    assert_eq!(read_only.entries[0].rights, 0x0012_0001);
    let receipt = permissions::security_receipt(&file).unwrap();
    assert_eq!(receipt["entries"][0]["rights"].as_u64(), Some(0x0012_0001));
    icacls(
        &file,
        &[
            "/inheritance:r".into(),
            "/grant:r".into(),
            format!("*{current}:F"),
            "*S-1-5-18:F".into(),
            "*S-1-5-32-544:F".into(),
        ],
    );
    let explicit = windows_acl::read(&file).unwrap();
    let receipt = permissions::security_receipt(&file).unwrap();
    assert_eq!(
        receipt,
        permissions::validate_windows_acl(&explicit.monitoring_value()).unwrap()
    );
    assert!(explicit.verify_mysql(false).is_err());
    icacls(&file, &["/grant".into(), "*S-1-1-0:R".into()]);
    assert!(permissions::security_receipt(&file).is_err());
    assert!(
        windows_acl::read(&file)
            .unwrap()
            .verify_mysql(false)
            .is_err()
    );
    icacls(
        &file,
        &[
            "/remove:g".into(),
            "*S-1-1-0".into(),
            "/deny".into(),
            // 仅拒绝写入数据，使安全句柄仍能读取这条拒绝 ACE。
            "*S-1-1-0:(WD)".into(),
        ],
    );
    let denied = windows_acl::read(&file).unwrap();
    assert!(
        denied
            .entries
            .iter()
            .any(|entry| entry.sid == "S-1-1-0" && entry.kind == 1 && entry.rights == 0x02)
    );
    assert!(permissions::security_receipt(&file).is_err());
    assert!(denied.verify_mysql(false).is_err());
    icacls(
        &file,
        &[
            "/remove:d".into(),
            "*S-1-1-0".into(),
            "/inheritance:e".into(),
        ],
    );
    assert!(permissions::security_receipt(&file).is_err());
    fs::remove_file(file).unwrap();
    fs::remove_dir(directory).unwrap();
}

#[test]
fn hidden_acl_failure_is_strict_json_and_does_not_echo_unknown_arguments() -> crate::Result<()> {
    let executable =
        option_env!("CARGO_BIN_EXE_xtask").ok_or("唯一 internal 入口必须绑定真实 xtask")?;
    let output = Command::new(executable)
        .args([
            "__windows-acl",
            "private-secret-invalid-operation",
            "private-secret-path",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(response["result"].is_null());
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("private-secret")
    );
    Ok(())
}
