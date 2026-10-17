use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::NaiveDate;
use serde_json::{Value, json};

use super::check::{
    CheckExecutionState, CheckPlanMode, PolicyProfile, TaskExecutionMode, TaskExecutor,
    VerifyExecutionContext,
    prerelease::{self, Finding, deployment, version},
};

fn oracle() -> Value {
    serde_json::from_str(include_str!("../fixtures/prerelease_legacy.json")).unwrap()
}

fn string<'a>(case: &'a Value, key: &str) -> &'a str {
    case[key].as_str().unwrap()
}

fn findings(value: &Value) -> BTreeSet<Finding> {
    serde_json::from_value(value.clone()).unwrap()
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 17).unwrap()
}

#[test]
fn prerelease_versions_and_literal_boundaries_match_python_oracle() {
    let oracle = oracle();
    for case in oracle["versions"].as_array().unwrap() {
        let value = string(case, "value");
        assert_eq!(
            version::is_version(value),
            case["version"].as_bool().unwrap(),
            "{case}"
        );
        assert_eq!(
            version::is_reference(value),
            case["reference"].as_bool().unwrap(),
            "{case}"
        );
    }
    for case in oracle["literals"].as_array().unwrap() {
        assert_eq!(
            json!(version::literals(string(case, "source"))),
            case["values"],
            "{case}"
        );
    }
}

#[test]
fn prerelease_scalars_containers_docker_and_resolution_match_python_oracle() {
    let oracle = oracle();
    for case in oracle["scalars"].as_array().unwrap() {
        assert_eq!(
            deployment::yaml_scalar(string(case, "source")),
            string(case, "value"),
            "{case}"
        );
    }
    for case in oracle["containers"].as_array().unwrap() {
        assert_eq!(
            json!(deployment::container(string(case, "source"))),
            case["value"],
            "{case}"
        );
    }
    for case in oracle["docker"].as_array().unwrap() {
        assert_eq!(
            json!(deployment::docker_image(string(case, "source"))),
            case["value"],
            "{case}"
        );
    }
    for case in oracle["resolve"].as_array().unwrap() {
        let vars: BTreeMap<String, String> =
            serde_json::from_value(case["variables"].clone()).unwrap();
        assert_eq!(
            deployment::resolve(string(case, "source"), &vars),
            string(case, "value"),
            "{case}"
        );
    }
}

#[test]
fn prerelease_deployment_scanning_matches_python_oracle() {
    for case in oracle()["deployment"].as_array().unwrap() {
        let source = string(case, "source");
        assert_eq!(
            json!(deployment::variables(source)),
            case["variables"],
            "{case}"
        );
        assert_eq!(
            deployment::findings(source, string(case, "path")),
            findings(&case["findings"]),
            "{case}"
        );
    }
}

#[test]
fn prerelease_allowlist_preserves_original_cases_with_two_strict_schema_rejections() {
    // 原始对照保持不变；这两项有意收紧 Python 布尔/浮点数与整数相等的旧缺口。
    let tightened = [
        r#"{"schema": true, "entries": []}"#,
        r#"{"schema": 1.0, "entries": []}"#,
    ];
    let mut tightened_count = 0;
    for case in oracle()["allowlists"].as_array().unwrap() {
        let source = string(case, "source");
        let parsed = prerelease::parse_allowlist(source);
        if tightened.contains(&source) {
            assert!(!case["allowed"].is_null(), "旧实现接受这两个输入");
            assert!(parsed.is_err(), "严格 schema 应拒绝：{case}");
            tightened_count += 1;
        } else if case["allowed"].is_null() {
            assert!(parsed.is_err(), "{case}");
        } else {
            let expected = case["allowed"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| {
                    let finding: Finding =
                        serde_json::from_value(entry["finding"].clone()).unwrap();
                    let date =
                        NaiveDate::parse_from_str(string(entry, "date"), "%Y-%m-%d").unwrap();
                    (finding, date)
                })
                .collect::<BTreeMap<_, _>>();
            assert_eq!(parsed.unwrap(), expected, "{case}");
        }
    }
    assert_eq!(tightened_count, 2);
    for invalid in ["", "{", "{\"schema\":1,\"entries\":[secret]}"] {
        let error = prerelease::parse_allowlist(invalid)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret"));
    }
}

#[test]
fn prerelease_allowlist_schema_accepts_only_json_integer_one() {
    assert!(
        prerelease::parse_allowlist(r#"{"schema":1,"entries":[]}"#)
            .unwrap()
            .is_empty()
    );
    for schema in [
        "true", "false", "1.0", "1e0", "null", r#""1""#, "-1", "0", "2",
    ] {
        let source = format!(r#"{{"schema":{schema},"entries":[]}}"#);
        let error = prerelease::parse_allowlist(&source)
            .unwrap_err()
            .to_string();
        assert!(error.contains("不支持的 schema"), "{schema}：{error}");
    }
    assert!(prerelease::parse_allowlist(r#"{"entries":[]}"#).is_err());
}

#[test]
fn prerelease_evaluation_preserves_sorting_stale_entries_and_inclusive_due_date() {
    let first = Finding::new("cargo", "a", "1.0.0-rc.1", "Cargo.lock");
    let second = Finding::new("cargo", "b", "1.0.0-beta", "Cargo.lock");
    let stale = Finding::new("tool", "removed", "nightly", "workflow.yaml");
    let values = BTreeSet::from([second.clone(), first.clone()]);
    let allowed = BTreeMap::from([(first.clone(), today()), (stale.clone(), today())]);
    let evaluation = prerelease::evaluate(&values, &allowed, today());
    assert_eq!(evaluation.unexpected, [second]);
    assert_eq!(evaluation.stale, [stale]);
    assert_eq!(evaluation.expired, [first]);
    assert!(
        prerelease::evaluate(&values, &allowed, today().pred_opt().unwrap())
            .expired
            .is_empty()
    );
    let allowed = values
        .iter()
        .cloned()
        .map(|finding| (finding, today().succ_opt().unwrap()))
        .collect();
    let evaluation = prerelease::evaluate(&values, &allowed, today());
    assert!(
        evaluation.unexpected.is_empty()
            && evaluation.stale.is_empty()
            && evaluation.expired.is_empty()
    );
}

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let parent = super::workspace::root_dir().join(".local-tests/prerelease-unit");
        fs::create_dir_all(&parent).unwrap();
        let root = parent.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let fixture = Self { root };
        fixture.write("Cargo.lock", "version = 4\n");
        fixture.write("Cargo.toml", "[workspace]\n");
        fixture.write(
            "scripts/prerelease_dependency_allowlist.json",
            "{\"schema\":1,\"entries\":[]}",
        );
        fixture
    }

    fn write(&self, relative: &str, value: &str) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files.insert(path.clone(), Vec::new());
                    visit(&path, files);
                } else {
                    files.insert(path.clone(), fs::read(path).unwrap());
                }
            }
        }
        let mut files = BTreeMap::new();
        visit(&self.root, &mut files);
        files
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(
            self.root.parent().unwrap(),
            super::workspace::root_dir().join(".local-tests/prerelease-unit")
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
fn prerelease_current_repository_findings_match_python_oracle() {
    let root = super::workspace::root_dir();
    assert_eq!(
        prerelease::collect(&root).unwrap(),
        findings(&oracle()["current"])
    );
    assert!(prerelease::check(&root, today()).unwrap().is_empty());
}

#[test]
fn prerelease_reads_real_locked_content_and_only_original_scan_locations() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.lock",
        "[[package]]\nname='locked'\nversion='1.2.3-rc.1'\n",
    );
    fixture.write("Cargo.toml", "# 2.0.0-beta\n");
    fixture.write("crates/demo/Cargo.toml", "# 3.0.0-preview\n");
    fixture.write("deploy/nested/Dockerfile.test", "FROM image:4.0.0-canary\n");
    fixture.write(
        ".github/workflows/test.yml",
        "uses: owner/action@v5.0.0-alpha\n",
    );
    fixture.write(".github/workflows/test.yaml", "image: image:nightly\n");
    for ignored in [
        "xtask/Cargo.toml",
        "crates/demo/nested/Cargo.toml",
        "deploy/compose.yml",
        ".github/workflows/nested/test.yml",
        ".github/workflows/test.txt",
        "Cargo.lock.backup",
    ] {
        fixture.write(ignored, "ignored: 9.0.0-rc.1");
    }
    let before = fixture.snapshot();
    let values = prerelease::collect(&fixture.root).unwrap();
    assert_eq!(values.len(), 6, "{values:?}");
    assert_eq!(fixture.snapshot(), before);
    let lock = fixture.root.join("Cargo.lock");
    let metadata = fs::metadata(&lock).unwrap();
    fixture.write(
        "Cargo.lock",
        "[[package]]\nname='locked'\nversion='1.2.3-beta'\n",
    );
    fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    assert_eq!(fs::metadata(&lock).unwrap().len(), metadata.len());
    assert_eq!(
        fs::metadata(&lock).unwrap().modified().unwrap(),
        metadata.modified().unwrap()
    );
    let after = prerelease::collect(&fixture.root).unwrap();
    assert!(after.contains(&Finding::new("cargo", "locked", "1.2.3-beta", "Cargo.lock")));
    assert!(!after.contains(&Finding::new("cargo", "locked", "1.2.3-rc.1", "Cargo.lock")));
}

#[test]
fn prerelease_malformed_inputs_fail_closed_without_printing_content_or_writing() {
    let fixture = Fixture::new();
    for (relative, source) in [
        ("Cargo.lock", "secret ' malformed"),
        ("Cargo.lock", "package = 1"),
        ("Cargo.lock", "[[package]]\nversion=2"),
        ("Cargo.lock", "[[package]]\nversion='1.2.3-beta'"),
        (
            "scripts/prerelease_dependency_allowlist.json",
            "[secret malformed",
        ),
    ] {
        let old = fs::read(fixture.root.join(relative)).unwrap();
        fixture.write(relative, source);
        let before = fixture.snapshot();
        let error = prerelease::check(&fixture.root, today())
            .unwrap_err()
            .to_string();
        assert!(
            !error.contains("secret")
                && !error.contains(&fixture.root.to_string_lossy().to_string())
        );
        assert_eq!(fixture.snapshot(), before);
        fs::write(fixture.root.join(relative), old).unwrap();
    }
    for relative in [
        "Cargo.lock",
        "Cargo.toml",
        "scripts/prerelease_dependency_allowlist.json",
    ] {
        let path = fixture.root.join(relative);
        let old = fs::read(&path).unwrap();
        fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(prerelease::check(&fixture.root, today()).is_err());
        fs::remove_file(&path).unwrap();
        assert!(prerelease::check(&fixture.root, today()).is_err());
        fs::create_dir(&path).unwrap();
        assert!(prerelease::check(&fixture.root, today()).is_err());
        fs::remove_dir(&path).unwrap();
        fs::write(&path, old).unwrap();
    }
}

#[test]
fn prerelease_diagnostics_are_deterministic_redacted_and_read_only() {
    let fixture = Fixture::new();
    fixture.write(
        ".github/workflows/test.yml",
        "image: https://user:supersecret@example.invalid/x:${MISSING}\nuses: $SECRET\n",
    );
    let before = fixture.snapshot();
    let errors = prerelease::check(&fixture.root, today()).unwrap();
    assert_eq!(errors.len(), 2);
    assert_eq!(prerelease::check(&fixture.root, today()).unwrap(), errors);
    assert!(!errors.join("\n").contains("supersecret"));
    assert!(!errors.join("\n").contains("SECRET"));
    assert_eq!(fixture.snapshot(), before);
    let exception = json!({"schema": 1, "entries": [{
        "ecosystem": "secret-ecosystem", "name": "secret-name", "version": "nightly",
        "source": "https://user:secret-source@example.invalid", "review_after": "2026-11-01",
        "reason": "secret-reason", "evidence": "https://user:secret-evidence@example.invalid"
    }]});
    fixture.write(
        "scripts/prerelease_dependency_allowlist.json",
        &exception.to_string(),
    );
    assert!(
        !prerelease::check(&fixture.root, today())
            .unwrap()
            .join("\n")
            .contains("secret-")
    );
}

#[test]
fn prerelease_rust_executor_and_full_ci_callers_reject_before_python() {
    let fixture = Fixture::new();
    // 身份残留检查的必需输入保持干净。
    fixture.write("openapi/openapi.json", "{}");
    fixture.write("sql/ryframe_config.sql", "");
    fixture.write(".github/workflows/test.yml", "image: x:nightly\n");
    let mut context = VerifyExecutionContext::new_ci(&fixture.root).unwrap();
    context.root = fixture.root.clone();
    context.include_frontend = false;
    let mut state = CheckExecutionState::default();
    let full = CheckPlanMode::ExplicitFull;
    let before = fixture.snapshot();
    for (executor, mode) in [
        (TaskExecutor::PrereleaseDependencies, TaskExecutionMode::Ci),
        (TaskExecutor::PolicyChecks, TaskExecutionMode::Ci),
        (TaskExecutor::PolicyChecks, TaskExecutionMode::Check(&full)),
        (
            TaskExecutor::BackendPolicyChecks,
            TaskExecutionMode::Check(&full),
        ),
    ] {
        let error = super::check::execute_registered_task(executor, mode, &context, &mut state)
            .unwrap_err();
        assert!(error.to_string().contains("未经评审"), "{error}");
        assert_eq!(super::process::failure_exit_code(error.as_ref()), Some(1));
        assert_eq!(fixture.snapshot(), before);
    }
    assert!(
        TaskExecutor::PrereleaseDependencies
            .definition()
            .allowed_writes
            .is_empty()
    );
    assert!(
        TaskExecutor::PrereleaseDependencies
            .definition()
            .external_resources
            .is_empty()
    );
    for profile in [
        PolicyProfile::FullStatic,
        PolicyProfile::CiPreflight,
        PolicyProfile::Smart,
    ] {
        assert!(
            super::check::policy_tasks(profile)
                .iter()
                .all(|task| task.id != "prerelease-dependencies")
        );
    }
}
