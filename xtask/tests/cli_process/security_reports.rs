use std::fs;

use super::xtask_command;

#[test]
fn cyclonedx_report_runs_only_its_registered_report_node() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let report_dir = root.join("target/cli process-供应链");
    fs::create_dir_all(&report_dir).unwrap();
    let report = report_dir.join(format!("cyclonedx-{}.json", std::process::id()));
    fs::write(
        &report,
        r#"{"bomFormat":"CycloneDX","specVersion":"1.5","metadata":{"component":{"name":"ryframe"}},"components":[{"name":"dependency"}]}"#,
    )
    .unwrap();
    let result = xtask_command()
        .env_remove("GITHUB_OUTPUT")
        .args([
            "check",
            "ci",
            "security",
            "report",
            "cyclonedx",
            "--input",
            report.to_str().unwrap(),
            "--require-reproducible",
        ])
        .output()
        .unwrap();
    let _ = fs::remove_file(&report);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(!stdout.contains("开始 CI 原子任务：python.environment"));
    assert!(stdout.contains("开始 CI 原子任务：ci.security.report.cyclonedx"));
    assert!(!stdout.contains("ci.security.supply-chain"));
    assert!(!stdout.contains("ci.security.audit"));
}

#[test]
fn trivy_report_runs_native_node_without_python_or_source_checks() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let report_dir = root.join("target/cli process-供应链");
    fs::create_dir_all(&report_dir).unwrap();
    let report = report_dir.join(format!("trivy-{}.json", std::process::id()));
    fs::write(&report, r#"{"Results":[]}"#).unwrap();
    let result = xtask_command()
        .env_remove("GITHUB_OUTPUT")
        .args([
            "check",
            "ci",
            "security",
            "report",
            "trivy",
            "--input",
            report.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let _ = fs::remove_file(&report);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(!stdout.contains("python.environment"));
    assert!(stdout.contains("开始 CI 原子任务：ci.security.report.trivy"));
    assert!(!stdout.contains("ci.security.supply-chain"));
    assert!(!stdout.contains("ci.security.audit"));
    assert!(stdout.contains("Trivy 报告校验通过"));

    fs::write(&report, r#"{"Results":{}}"#).unwrap();
    let failure = xtask_command()
        .env_remove("GITHUB_OUTPUT")
        .args([
            "check",
            "ci",
            "security",
            "report",
            "trivy",
            "--input",
            report.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let _ = fs::remove_file(&report);
    assert_eq!(failure.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&failure.stderr);
    assert!(stderr.contains("Trivy 报告缺少 Results 数组"), "{stderr}");
    assert!(!stderr.contains("trivy-"));
    assert!(!String::from_utf8_lossy(&failure.stdout).contains("python.environment"));
}

#[test]
fn cyclonedx_report_failures_preserve_exit_code_and_redact_input_without_python() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let directory = root.join("target/cli process-供应链");
    fs::create_dir_all(&directory).unwrap();
    let report = directory.join(format!("secret-report-failure-{}.json", std::process::id()));
    let inputs: [Option<&[u8]>; 4] = [
        None,
        Some(b"{secret-token}"),
        Some(&[0xff, 0xfe]),
        Some(br#"{"bomFormat":"secret-token"}"#),
    ];
    for input in inputs {
        if let Some(input) = input {
            fs::write(&report, input).unwrap();
        }
        let output = xtask_command()
            .env_remove("GITHUB_OUTPUT")
            .args([
                "check",
                "ci",
                "security",
                "report",
                "cyclonedx",
                "--input",
                report.to_str().unwrap(),
                "--require-reproducible",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("secret-token") && !stderr.contains("secret-report"),
            "{stderr}"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("python.environment"));
        if let Some(input) = input {
            assert_eq!(fs::read(&report).unwrap(), input);
        } else {
            assert!(!report.exists());
        }
    }
    fs::remove_file(report).unwrap();
}
