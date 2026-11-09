use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use super::{
    check::python_environment::{
        EnvironmentReport, Requirement, decode_report, parse_requirements, validate_environment,
    },
    process,
};

const TIMEOUT_HELPER: &str = "python_environment_tests::timeout_probe_helper";

#[test]
fn requirements_parser_reads_exact_versions_across_hash_continuations() {
    let source = concat!(
        "First.Package==1.2.3 \\\n",
        "    --hash=sha256:",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        "second==4.5.6\n",
    );
    assert_eq!(
        parse_requirements("requirements.txt", source).unwrap(),
        [
            Requirement {
                name: "First.Package".into(),
                version: "1.2.3".into(),
            },
            Requirement {
                name: "second".into(),
                version: "4.5.6".into(),
            },
        ]
    );
}

#[test]
fn requirements_parser_rejects_unknown_duplicate_and_non_exact_entries() {
    for (source, expected) in [
        ("requests>=2.0\n", "严格 name==version"),
        (
            "requests==2.0 --index-url https://invalid\n",
            "未知 requirements 选项",
        ),
        ("demo==1\ndemo==2\n", "重复声明"),
        ("demo==1 \\\n\n", "续行不能为空"),
        (
            "demo==1 --hash=sha256:abcd\n",
            "sha256 摘要必须为 64 位十六进制",
        ),
        ("", "固定 Python 依赖为空"),
    ] {
        let error = parse_requirements("fixture.txt", source).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "期望包含 {expected:?}，实际为 {error}"
        );
    }
}

#[test]
fn environment_protocol_reports_old_python_and_dependency_mismatches() {
    let requirements = fixture_requirements();
    let old = report([3, 10, 12], &[("PyYAML", Some("6.0.3"))]);
    let error = validate_environment(&requirements, [3, 11, 0], &old).unwrap_err();
    assert!(error.to_string().contains("Python 版本过低"));

    let mismatched = report([3, 12, 7], &[("PyYAML", Some("6.0.2"))]);
    let error = validate_environment(&requirements, [3, 11, 0], &mismatched).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("Python 依赖版本不匹配：PyYAML"));
    let missing = report([3, 12, 7], &[("PyYAML", None)]);
    assert!(
        validate_environment(&requirements, [3, 11, 0], &missing)
            .unwrap_err()
            .to_string()
            .contains("缺少固定 Python 依赖：PyYAML==6.0.3")
    );
}

#[test]
fn environment_protocol_rejects_import_failure_and_incomplete_identity() {
    let requirements = fixture_requirements();
    let mut observed = report([3, 12, 7], &[("PyYAML", Some("6.0.3"))]);
    observed.errors.push("PyYAML import: ImportError".into());
    assert!(
        validate_environment(&requirements, [3, 11, 0], &observed)
            .unwrap_err()
            .to_string()
            .contains("PyYAML import")
    );
    observed.errors.clear();
    observed.executable.clear();
    observed.implementation.clear();
    assert!(validate_environment(&requirements, [3, 11, 0], &observed).is_err());
}

#[test]
fn environment_protocol_accepts_exact_utf8_report_and_rejects_extra_fields() {
    let valid = r#"{
        "executable":"D:\\Python 环境\\python.exe",
        "implementation":"CPython",
        "version":[3,12,7],
        "distribution_versions":{
            "PyYAML":"6.0.3"
        },
        "errors":[]
    }"#;
    let report = decode_report(valid.as_bytes()).unwrap();
    assert_eq!(report.executable, "D:\\Python 环境\\python.exe");
    assert_eq!(report.version, [3, 12, 7]);

    let extra = valid.trim_end_matches('}').to_owned() + r#","unknown":true}"#;
    assert!(decode_report(extra.as_bytes()).is_err());
    assert!(decode_report(&[0xff]).is_err());
    assert!(decode_report(b"").is_err());
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
#[ignore = "需要已按 scripts/requirements-ci.txt 安装固定 Python 依赖"]
fn current_workspace_python_environment_executor_passes() {
    super::check::python_environment::run(&super::workspace::root_dir()).unwrap();
}

fn fixture_requirements() -> Vec<Requirement> {
    [("PyYAML", "6.0.3")]
        .into_iter()
        .map(|(name, version)| Requirement {
            name: name.into(),
            version: version.into(),
        })
        .collect()
}

fn report(version: [u64; 3], versions: &[(&str, Option<&str>)]) -> EnvironmentReport {
    EnvironmentReport {
        executable: "D:\\Python 环境\\python.exe".into(),
        implementation: "CPython".into(),
        version: version.into(),
        distribution_versions: versions
            .iter()
            .map(|(name, version)| ((*name).into(), version.map(str::to_owned)))
            .collect::<BTreeMap<_, _>>(),
        errors: Vec::new(),
    }
}
