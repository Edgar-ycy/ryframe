use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    check::trivy,
    ci,
    cli::{CiCommand, SecurityCommand, SecurityReportKind, SecurityReportOptions},
    process::failure_exit_code,
};

const EMPTY_POLICY: &str =
    r#"{"vulnerability_gate":{"severities":["HIGH","CRITICAL"],"exceptions":[]}}"#;
const POLICY_WITH_EXCEPTION: &str = r#"{
  "vulnerability_gate": {
    "severities": ["HIGH", "CRITICAL"],
    "exceptions": [
      {
        "id": "CVE-2099-0001",
        "package": "libexample",
        "installed_version": "1.0.0",
        "owner": "security-team",
        "expires": "2099-12-31",
        "reason": "等待上游发布兼容的安全修复版本"
      }
    ]
  }
}"#;
const REPORT_WITH_EXCEPTION: &str = r#"{
  "Results": [
    {
      "Target": "runtime",
      "Vulnerabilities": [
        {
          "VulnerabilityID": "CVE-2099-0001",
          "PkgName": "libexample",
          "InstalledVersion": "1.0.0",
          "Severity": "CRITICAL"
        }
      ]
    }
  ]
}"#;
const EMPTY_REPORT: &str = r#"{"Results":[{"Target":"runtime","Vulnerabilities":[]}]}"#;

#[test]
fn trivy_matches_exact_package_version_and_rejects_unused_exception() {
    let policy = trivy::parse_policy(POLICY_WITH_EXCEPTION).unwrap();
    assert!(trivy::validate(REPORT_WITH_EXCEPTION, &policy).is_empty());

    let errors = trivy::validate(EMPTY_REPORT, &policy);
    assert_eq!(
        errors,
        ["漏洞例外未被报告使用，必须删除或校正：CVE-2099-0001 libexample@1.0.0"]
    );

    let wrong_package = REPORT_WITH_EXCEPTION.replace(
        r#""PkgName": "libexample""#,
        r#""PkgName": "another-package""#,
    );
    let errors = trivy::validate(&wrong_package, &policy);
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("未放行的 CRITICAL 漏洞"))
    );
    assert!(
        errors
            .iter()
            .any(|error| error.contains("漏洞例外未被报告使用"))
    );

    let wrong_version = REPORT_WITH_EXCEPTION.replace(
        r#""InstalledVersion": "1.0.0""#,
        r#""InstalledVersion": "1.0.1""#,
    );
    let errors = trivy::validate(&wrong_version, &policy);
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("未放行的 CRITICAL 漏洞"))
    );

    let target_policy = trivy::parse_policy(&POLICY_WITH_EXCEPTION.replace(
        r#""owner": "security-team","#,
        r#""owner": "security-team", "target": "runtime","#,
    ))
    .unwrap();
    assert!(trivy::validate(REPORT_WITH_EXCEPTION, &target_policy).is_empty());
    let other_target =
        REPORT_WITH_EXCEPTION.replace(r#""Target": "runtime""#, r#""Target": "other""#);
    let errors = trivy::validate(&other_target, &target_policy);
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("未放行的 CRITICAL 漏洞"))
    );
}

#[test]
fn trivy_rejects_invalid_policy_and_report_shapes() {
    for source in [
        "",
        "{}",
        r#"{"vulnerability_gate":null}"#,
        r#"{"vulnerability_gate":{"severities":[],"exceptions":[{"id":1}]}}"#,
    ] {
        assert!(trivy::parse_policy(source).is_err(), "{source}");
    }
    for source in [
        EMPTY_POLICY.replace(r#""exceptions":[]"#, r#""exceptions":[],"unknown":true"#),
        POLICY_WITH_EXCEPTION.replace("2099-12-31", "2000-01-01"),
        POLICY_WITH_EXCEPTION.replace(r#""owner": "security-team""#, r#""owner": "x""#),
    ] {
        assert!(trivy::parse_policy(&source).is_err(), "{source}");
    }
    for advisory_id in ["A?", "C VE", "A"] {
        let source = POLICY_WITH_EXCEPTION.replace("CVE-2099-0001", advisory_id);
        assert!(trivy::parse_policy(&source).is_err(), "{advisory_id}");
    }
    let shortest = POLICY_WITH_EXCEPTION.replace("CVE-2099-0001", "A1");
    assert!(trivy::parse_policy(&shortest).is_ok());

    let policy = trivy::parse_policy(EMPTY_POLICY).unwrap();
    for (source, expected) in [
        ("", "无法读取 Trivy 报告"),
        ("[]", "无法读取 Trivy 报告"),
        ("{}", "Trivy 报告缺少 Results 数组"),
        (r#"{"Results":{}}"#, "Trivy 报告缺少 Results 数组"),
        (r#"{"Results":[null]}"#, "Trivy Results 包含非对象条目"),
        (
            r#"{"Results":[{"Target":"runtime","Vulnerabilities":{}}]}"#,
            "Vulnerabilities 不是数组",
        ),
        (
            r#"{"Results":[{"Target":"runtime","Vulnerabilities":[null]}]}"#,
            "包含无效漏洞条目",
        ),
        (
            r#"{"Results":[{"Target":"runtime","Vulnerabilities":[{"Severity":"CRITICAL"}]}]}"#,
            "高等级漏洞缺少字符串字段",
        ),
    ] {
        let errors = trivy::validate(source, &policy);
        assert_eq!(errors.len(), 1, "{source}");
        assert!(errors[0].contains(expected), "{source}: {errors:?}");
    }

    let low = r#"{"Results":[{"Target":"runtime","Vulnerabilities":[{"Severity":"LOW"}]}]}"#;
    assert!(trivy::validate(low, &policy).is_empty());
}

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let parent = super::workspace::root_dir().join(".local-tests/trivy-unit");
        fs::create_dir_all(&parent).unwrap();
        let root = parent.join(format!(
            "报告 包含空格-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir_all(root.join("tools/policies")).unwrap();
        fs::write(
            root.join("tools/policies/supply_chain_policy.json"),
            include_str!("../../../tools/policies/supply_chain_policy.json")
                .replace("2026-12-31", "2099-12-31"),
        )
        .unwrap();
        fs::write(root.join("report.json"), EMPTY_REPORT).unwrap();
        fs::write(root.join("unrelated.txt"), "不得覆盖").unwrap();
        Self { root }
    }

    fn report(&self) -> PathBuf {
        self.root.join("report.json")
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn collect(root: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(root).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    collect(&path, output);
                } else {
                    output.insert(path.clone(), fs::read(path).unwrap());
                }
            }
        }

        let mut output = BTreeMap::new();
        collect(&self.root, &mut output);
        output
    }

    fn options(&self, input: PathBuf) -> SecurityReportOptions {
        SecurityReportOptions {
            kind: SecurityReportKind::Trivy,
            input,
            require_reproducible: false,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(
            self.root.parent().unwrap(),
            super::workspace::root_dir().join(".local-tests/trivy-unit")
        );
        assert!(
            !fs::symlink_metadata(&self.root)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn trivy_check_and_actual_ci_entry_are_read_only_without_python_or_source() {
    let fixture = Fixture::new();
    let before = fixture.snapshot();
    trivy::run(&fixture.report(), &fixture.root).unwrap();
    ci::run(
        CiCommand::Security(SecurityCommand::Report(fixture.options(fixture.report()))),
        &fixture.root.join("frontend-absent"),
    )
    .unwrap();
    assert_eq!(fixture.snapshot(), before);

    let missing = fixture.root.join("must-not-be-created/secret-report.json");
    let command = CiCommand::Security(SecurityCommand::Report(fixture.options(missing.clone())));
    let plan = ci::ci_execution_plan_for(&command).unwrap();
    assert_eq!(plan.tasks.len(), 1);
    assert_eq!(fixture.snapshot(), before);
    let error = ci::run(command, &fixture.root).unwrap_err();
    assert_eq!(failure_exit_code(error.as_ref()), Some(1));
    assert!(!error.to_string().contains("secret-report"));
    assert!(!missing.parent().unwrap().exists());
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn trivy_file_errors_and_invalid_utf8_preserve_failure_code_without_writes() {
    let fixture = Fixture::new();
    let policy = trivy::parse_policy(EMPTY_POLICY).unwrap();
    let path = fixture.report();
    fs::write(&path, [0xff, 0xfe]).unwrap();
    let before = fixture.snapshot();
    let error = trivy::run(&path, &fixture.root).unwrap_err();
    assert_eq!(failure_exit_code(error.as_ref()), Some(1));
    assert_eq!(fixture.snapshot(), before);

    fs::remove_file(&path).unwrap();
    for input in [&path, fixture.root.as_path()] {
        let error = trivy::check(input, &policy).unwrap_err();
        assert_eq!(failure_exit_code(error.as_ref()), Some(1));
        assert!(!error.to_string().contains(input.to_str().unwrap()));
    }
}
