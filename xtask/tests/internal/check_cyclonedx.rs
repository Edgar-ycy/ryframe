use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

use super::{
    check::cyclonedx,
    ci,
    cli::{CiCommand, SecurityCommand, SecurityReportKind, SecurityReportOptions},
    process::failure_exit_code,
};

const VALID: &str = r#"{"bomFormat":"CycloneDX","specVersion":"1.5","metadata":{"component":{"name":"ryframe"}},"components":[{"name":"dependency"}]}"#;

#[test]
fn cyclonedx_matches_all_original_report_cases_and_rejects_legacy_type_errors() {
    let oracle: Value =
        serde_json::from_str(include_str!("../fixtures/cyclonedx_legacy.json")).unwrap();
    let cases = oracle["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 154);
    let mut legacy_type_errors = 0;
    for case in cases {
        let source = case["source"].as_str().unwrap();
        let reproducible = case["require_reproducible"].as_bool().unwrap();
        let errors = cyclonedx::validate(source, reproducible);
        assert_eq!(
            errors.is_empty(),
            case["accepted"].as_bool().unwrap(),
            "{case}"
        );
        if case["errors"].is_null() {
            // 旧 Python 在不可哈希 specVersion 上异常退出；Rust 同样拒绝并返回稳定错误。
            assert!(!errors.is_empty());
            legacy_type_errors += 1;
        } else {
            assert_eq!(serde_json::json!(errors), case["errors"], "{case}");
        }
    }
    assert_eq!(legacy_type_errors, 6);
}

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let parent = super::workspace::root_dir().join(".local-tests/cyclonedx-unit");
        fs::create_dir_all(&parent).unwrap();
        let root = parent.join(format!(
            "报告 包含空格-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("report.json"), VALID).unwrap();
        fs::write(root.join("unrelated.txt"), "不得覆盖").unwrap();
        Self { root }
    }

    fn report(&self) -> PathBuf {
        self.root.join("report.json")
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fs::read_dir(&self.root)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let content = fs::read(&path).unwrap();
                (path, content)
            })
            .collect()
    }

    fn options(&self, input: PathBuf) -> SecurityReportOptions {
        SecurityReportOptions {
            kind: SecurityReportKind::CycloneDx,
            input,
            require_reproducible: true,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(
            self.root.parent().unwrap(),
            super::workspace::root_dir().join(".local-tests/cyclonedx-unit")
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
fn cyclonedx_check_and_actual_ci_entry_are_read_only_without_policy_or_frontend() {
    let fixture = Fixture::new();
    let before = fixture.snapshot();
    assert!(
        cyclonedx::check(&fixture.report(), true)
            .unwrap()
            .is_empty()
    );
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
fn cyclonedx_reads_changed_contents_even_when_length_and_mtime_match() {
    let fixture = Fixture::new();
    let path = fixture.report();
    let metadata = fs::metadata(&path).unwrap();
    assert!(cyclonedx::check(&path, true).unwrap().is_empty());
    fs::write(&path, VALID.replace("1.5", "1.4")).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), metadata.len());
    assert_eq!(
        fs::metadata(&path).unwrap().modified().unwrap(),
        metadata.modified().unwrap()
    );
    let before = fixture.snapshot();
    let errors = cyclonedx::check(&path, true).unwrap();
    assert_eq!(errors, ["SBOM 规范版本不得低于 1.5"]);
    let command = CiCommand::Security(SecurityCommand::Report(fixture.options(path)));
    let error = ci::run(command, &fixture.root).unwrap_err();
    assert_eq!(failure_exit_code(error.as_ref()), Some(1));
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn cyclonedx_invalid_json_and_field_values_never_appear_in_diagnostics() {
    for source in [
        "",
        "{",
        "{secret-token}",
        "\u{feff}{}",
        "[]",
        "null",
        "true",
        r#"{"bomFormat":"secret-token","specVersion":["secret-token"],"components":"secret-token","metadata":"secret-token","serialNumber":"secret-token"}"#,
    ] {
        let errors = cyclonedx::validate(source, true);
        assert!(!errors.is_empty(), "{source}");
        assert!(!errors.join("\n").contains("secret-token"));
        assert_eq!(cyclonedx::validate(source, true), errors);
    }
}

#[test]
fn cyclonedx_file_errors_and_invalid_utf8_preserve_failure_code_without_writes() {
    let fixture = Fixture::new();
    let path = fixture.report();
    fs::write(&path, [0xff, 0xfe]).unwrap();
    let before = fixture.snapshot();
    let error = cyclonedx::run(&path, true).unwrap_err();
    assert_eq!(failure_exit_code(error.as_ref()), Some(1));
    assert_eq!(fixture.snapshot(), before);
    fs::remove_file(&path).unwrap();
    for input in [&path, fixture.root.as_path()] {
        let error = cyclonedx::run(input, true).unwrap_err();
        assert_eq!(failure_exit_code(error.as_ref()), Some(1));
        assert!(!error.to_string().contains(input.to_str().unwrap()));
    }
}
