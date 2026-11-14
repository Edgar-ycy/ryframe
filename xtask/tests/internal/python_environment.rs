use std::{
    process::{ExitStatus, Output},
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(windows)]
use std::os::windows::process::ExitStatusExt;

use super::{MINIMUM_PYTHON, environment_report, protocol, run};
use crate::process;

const TIMEOUT_HELPER: &str = "check::python_environment::tests::timeout_probe_helper";
const REPORT: &str = r#"{
    "executable":"D:\\Python 环境\\python.exe",
    "implementation":"CPython",
    "version":[3,12,7]
}"#;

#[test]
fn environment_protocol_validates_minimum_version_and_identity() {
    let mut report = protocol::decode_report(REPORT.as_bytes(), b"").unwrap();
    for accepted in [[3, 11, 0], [3, 12, 7]] {
        report.version = accepted.into();
        protocol::validate_environment(MINIMUM_PYTHON, &report).unwrap();
    }

    report.version = vec![3, 10, 12];
    let error = protocol::validate_environment(MINIMUM_PYTHON, &report).unwrap_err();
    assert!(error.to_string().contains("Python 版本过低"));
    assert_eq!(process::failure_exit_code(error.as_ref()), Some(1));

    report.version.truncate(2);
    report.executable.clear();
    report.implementation.clear();
    let error = protocol::validate_environment(MINIMUM_PYTHON, &report).unwrap_err();
    let message = error.to_string();
    for expected in ["实际解释器路径", "解释器实现", "major.minor.patch"] {
        assert!(message.contains(expected), "{message}");
    }
}

#[test]
fn environment_protocol_accepts_exact_utf8_report_and_rejects_extra_fields() {
    let report = protocol::decode_report(REPORT.as_bytes(), "提示".as_bytes()).unwrap();
    assert_eq!(report.executable, "D:\\Python 环境\\python.exe");
    assert_eq!(report.implementation, "CPython");
    assert_eq!(report.version, [3, 12, 7]);

    let extra = REPORT.trim_end_matches('}').to_owned() + r#","unknown":true}"#;
    for (output, error_output, expected) in [
        (extra.as_bytes(), b"".as_slice(), "不是有效 JSON"),
        (b"{".as_slice(), b"".as_slice(), "不是有效 JSON"),
        (&[0xff][..], b"".as_slice(), "输出不是 UTF-8"),
        (REPORT.as_bytes(), &[0xff][..], "标准错误不是 UTF-8"),
        (b"".as_slice(), b"".as_slice(), "没有输出"),
    ] {
        let output = Output {
            status: ExitStatus::from_raw(0),
            stdout: output.to_vec(),
            stderr: error_output.to_vec(),
        };
        let error = environment_report(&output, "测试解释器").unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(process::failure_exit_code(error.as_ref()), Some(1));
    }
}

#[test]
fn nonzero_probe_preserves_original_process_exit_and_stderr() {
    #[cfg(unix)]
    let status = ExitStatus::from_raw(23 << 8);
    #[cfg(windows)]
    let status = ExitStatus::from_raw(0xc000_0005);

    let output = Output {
        status,
        stdout: b"unexpected stdout".to_vec(),
        stderr: "探针失败".as_bytes().to_vec(),
    };
    let error = environment_report(&output, "测试解释器").unwrap_err();
    let message = error.to_string();
    assert_eq!(process::failure_exit_code(error.as_ref()), status.code());
    assert!(
        message.contains("探针失败") && message.contains("测试解释器"),
        "{message}"
    );
    assert!(!message.contains("unexpected stdout"), "{message}");
}

#[test]
fn missing_python_process_reports_the_exact_unicode_path() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("不存在-python.exe");
    let executable = missing.to_str().unwrap();
    let error = process::command_output_with_input_timeout(
        directory.path(),
        executable,
        &["-I", "-c", "pass"],
        &[],
        Some(b"{}"),
        Duration::from_secs(1),
    )
    .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("无法执行一次性子进程输出协议"), "{text}");
    assert!(text.contains("不存在-python.exe"), "{text}");
}

#[test]
fn probe_timeout_terminates_the_helper_process_tree() {
    let started = Instant::now();
    let executable = std::env::current_exe().unwrap();
    let executable = executable.to_str().unwrap();
    let error = process::command_output_with_input_timeout(
        &std::env::temp_dir(),
        executable,
        &[TIMEOUT_HELPER, "--exact", "--ignored", "--nocapture"],
        &[],
        None,
        Duration::from_millis(100),
    )
    .unwrap_err();
    assert!(error.to_string().contains("未退出"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "超时后必须立即回收探针进程树"
    );
}

#[test]
#[ignore = "仅由 Python 环境探针超时测试作为子进程调用"]
fn timeout_probe_helper() {
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
fn current_workspace_python_environment_executor_passes() {
    run(&crate::workspace::root_dir()).unwrap();
}
