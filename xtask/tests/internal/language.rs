use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    check::{
        CheckPlanMode, TaskExecutor,
        language::{
            LanguageOptions, Percent, git, inventory,
            model::{Kind, ratio_text},
            plan_at, policy, report, run_with_git,
        },
        tasks_for,
    },
    cli::{CheckCommand, CheckScope, Command as CliCommand, parse},
    process::failure_exit_code,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = parent.join(format!("language-{}-{id}-{nonce}", std::process::id()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn write(&self, path: &str, bytes: impl AsRef<[u8]>) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn init_git(&self) {
        self.git(&["init", "--quiet"]);
        self.write(".gitignore", ".local-tests/\nignored/\n");
    }

    fn git(&self, arguments: &[&str]) {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(&self.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
        fn walk(root: &Path, directory: &Path, files: &mut Vec<(String, Vec<u8>)>) {
            for entry in fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, files);
                } else {
                    files.push((
                        path.strip_prefix(root).unwrap().display().to_string(),
                        fs::read(path).unwrap(),
                    ));
                }
            }
        }
        let mut files = Vec::new();
        walk(&self.0, &self.0, &mut files);
        files.sort();
        files
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let boundary = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        assert_eq!(self.0.parent(), Some(boundary.as_path()));
        assert!(
            !fs::symlink_metadata(&self.0)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn parsed(arguments: &[&str]) -> Result<LanguageOptions, String> {
    let args = [
        vec!["check".to_owned(), "language".to_owned()],
        arguments.iter().map(|value| (*value).into()).collect(),
    ]
    .concat();
    match parse(args).map_err(|error| error.to_string())?.command {
        CliCommand::Check(CheckCommand::Language(options)) => Ok(options),
        _ => Err("没有解析为语言命令".into()),
    }
}

#[test]
fn policy_covers_all_declared_extensions_without_collisions() {
    assert_eq!(
        policy::digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let rules = policy::load().unwrap();
    for (language, extensions) in &rules.source_extensions {
        for extension in extensions {
            let file = policy::classify(&rules, &format!("src/中文 空格.{extension}"), b"source\n")
                .unwrap();
            assert_eq!(file.kind, Kind::Code);
            assert_eq!(file.language.as_deref(), Some(language.as_str()));
        }
    }
    for (kind, extensions) in [
        (Kind::Document, &rules.document_extensions),
        (Kind::Config, &rules.config_extensions),
        (Kind::Resource, &rules.resource_extensions),
    ] {
        for extension in extensions {
            assert_eq!(
                policy::classify(&rules, &format!("file.{extension}"), b"data")
                    .unwrap()
                    .kind,
                kind
            );
        }
    }
    assert!(rules.legacy_reference.contains("不可复现"));
    assert!(rules.legacy_reference.contains("不与本口径计算趋势"));
}

#[test]
fn classification_keeps_tests_generated_code_templates_and_binary_volume() {
    let rules = policy::load().unwrap();
    for (path, language, generated, test, template) in [
        ("src/lib.rs", "Rust", false, false, false),
        ("tests/中文 空格.rs", "Rust", false, true, false),
        (
            "crates/api/src/generated/lib.rs",
            "Rust",
            true,
            false,
            false,
        ),
        ("tests/generated/lib.rs", "Rust", true, true, false),
        ("src/generated_router.rs", "Rust", true, false, false),
        ("src/router.generated.rs", "Rust", true, false, false),
        ("templates/handler.rs.tpl", "Rust", false, false, true),
        ("templates/handler.rs.tpl.hbs", "Rust", false, false, true),
        ("templates/page.vue.j2", "Vue", false, false, true),
        ("templates/plain.hbs", "Template", false, false, true),
        ("tools/test_guard.py", "Python", false, true, false),
        ("tools/guard.test.mjs", "JavaScript", false, true, false),
        ("src/LIB.RS", "Rust", false, false, false),
        ("deploy/Dockerfile.api", "Build", false, false, false),
        ("scripts/extensionless", "Other", false, false, false),
    ] {
        let file = policy::classify(&rules, path, b"source").unwrap();
        assert_eq!(file.language.as_deref(), Some(language), "{path}");
        assert_eq!(
            (file.generated, file.test, file.template),
            (generated, test, template),
            "{path}"
        );
        assert_eq!(file.bytes, 6);
    }
    for path in ["vendor/third/src/lib.rs", "vendor/lib.py"] {
        assert!(
            policy::classify(&rules, path, b"third party")
                .unwrap()
                .vendor
        );
    }
    assert!(
        !policy::classify(&rules, "src/vendor/lib.rs", b"owned")
            .unwrap()
            .vendor
    );
    let binary = policy::classify(&rules, "assets/data.unknown", &[0, 255, 1]).unwrap();
    assert_eq!((binary.kind, binary.bytes), (Kind::Resource, 3));
    assert!(policy::classify(&rules, "src/lib.rs", &[0, 255]).is_err());
    assert!(policy::classify(&rules, "templates/lib.rs.tpl", &[255]).is_err());
    let config = policy::classify(&rules, "config/tools.toml.tpl", b"x = 1").unwrap();
    assert_eq!(config.kind, Kind::Config);
    assert!(config.template);
}

#[test]
fn generated_marker_only_matches_header_line_prefixes() {
    let rules = policy::load().unwrap();
    assert!(
        policy::classify(&rules, "src/lib.rs", b"// @generated by RyFrame 1\n")
            .unwrap()
            .generated
    );
    assert!(
        !policy::classify(
            &rules,
            "src/lib.rs",
            b"let marker = \"// @generated by RyFrame\";\n"
        )
        .unwrap()
        .generated
    );
    let mut bytes = vec![b' '; 2049];
    bytes.extend_from_slice(b"\n// @generated by RyFrame\n");
    assert!(
        !policy::classify(&rules, "src/lib.rs", &bytes)
            .unwrap()
            .generated
    );
}

#[test]
fn rust_threshold_uses_exact_integer_math_without_rounding_or_overflow() {
    let eighty = Percent::parse("80").unwrap();
    assert!(eighty.accepts(4, 5));
    assert!(!eighty.accepts(79_999_999, 100_000_000));
    assert!(eighty.accepts(80_000_000, 100_000_000));
    assert_eq!(
        ratio_text(79_999_999, 100_000_000).as_deref(),
        Some("79.999999")
    );
    assert!(!Percent::parse("66.666667").unwrap().accepts(2, 3));
    assert!(Percent::parse("66.666666").unwrap().accepts(2, 3));
    assert!(
        Percent::parse("100.000000")
            .unwrap()
            .accepts(u64::MAX, u64::MAX)
    );
    assert!(
        !Percent::parse("100")
            .unwrap()
            .accepts(u64::MAX - 1, u64::MAX)
    );
    assert!(!Percent::parse("0").unwrap().accepts(0, 0));
    assert_eq!(ratio_text(0, 0), None);
    for invalid in [
        "",
        ".1",
        "80.",
        "-1",
        "+80",
        "NaN",
        "inf",
        "8e1",
        "100.000001",
        "80.0000001",
        " 80",
        "4294967296",
    ] {
        assert!(Percent::parse(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn report_keeps_vendor_separate_and_all_own_code_in_the_denominator() {
    let rules = policy::load().unwrap();
    let files = [
        ("src/lib.rs", 40),
        ("tests/test.rs", 20),
        ("src/generated/mod.rs", 10),
        ("templates/lib.rs.tpl", 10),
        ("tools/check.py", 20),
        ("vendor/lib.rs", 500),
        ("README.md", 10),
        ("config/tools.toml", 20),
        ("assets/logo.png", 30),
    ]
    .into_iter()
    .map(|(path, size)| policy::classify(&rules, path, &vec![b'x'; size]).unwrap())
    .collect();
    let empty = inventory::Inventory {
        head: None,
        eligible: BTreeSet::new(),
        ignored: BTreeSet::new(),
        deleted: BTreeSet::new(),
    };
    let report = report::build(rules, empty, files, Some(Percent::parse("80").unwrap())).unwrap();
    assert_eq!(report.total.bytes, 660);
    assert_eq!(report.self_maintained.code.bytes, 100);
    assert_eq!(report.self_maintained.rust.bytes, 80);
    assert_eq!(report.vendor.code.bytes, 500);
    assert_eq!(report.threshold_passed, Some(true));
    assert_eq!(report.rust_percent.as_deref(), Some("80.000000"));
    assert_eq!(report.self_maintained.code_groups["测试"].bytes, 20);
    assert_eq!(report.self_maintained.code_groups["生成"].bytes, 10);
    assert_eq!(report.self_maintained.code_groups["模板"].bytes, 10);
}

#[test]
fn real_git_excludes_ignored_tracked_and_untracked_files_without_writing() {
    let fixture = Fixture::new();
    fixture.init_git();
    fixture.write("ignored/已跟踪.rs", b"not counted");
    fixture.git(&["add", "-f", "ignored/已跟踪.rs"]);
    fixture.write("ignored/untracked.py", b"not counted");
    fixture.write("src/中文 空格.rs", b"owned Rust");
    fixture.write("vendor/lib.rs", b"vendor Rust");
    fixture.write("deleted.rs", b"deleted");
    fixture.git(&["add", "deleted.rs"]);
    fs::remove_file(fixture.0.join("deleted.rs")).unwrap();
    let before = fixture.snapshot();
    let mut git = |args: &[&str]| git::invoke(&fixture.0, args);
    let inventory = inventory::discover(&fixture.0, &mut git).unwrap();
    assert_eq!(
        inventory.ignored,
        BTreeSet::from(["ignored/已跟踪.rs".into()])
    );
    assert_eq!(inventory.deleted, BTreeSet::from(["deleted.rs".into()]));
    assert_eq!(
        inventory.eligible,
        BTreeSet::from([
            ".gitignore".into(),
            "src/中文 空格.rs".into(),
            "vendor/lib.rs".into()
        ])
    );
    run_with_git(&fixture.0, &LanguageOptions::default(), &mut git).unwrap();
    assert_eq!(before, fixture.snapshot());
}

#[test]
fn plan_uses_the_execution_graph_and_never_invokes_git_or_creates_directories() {
    let fixture = Fixture::new();
    let options = LanguageOptions {
        min_rust_percent: Some(Percent::parse("80").unwrap()),
        output: Some(".local-tests/missing/report.json".into()),
        write: true,
        plan: true,
    };
    let plan = plan_at(&fixture.0, &options).unwrap();
    assert_eq!(
        plan.tasks
            .iter()
            .map(|task| task.executor)
            .collect::<Vec<_>>(),
        [
            TaskExecutor::LanguageReport,
            TaskExecutor::LanguageWrite,
            TaskExecutor::LanguageThreshold
        ]
    );
    let mut executable = options.clone();
    executable.plan = false;
    assert_eq!(plan, plan_at(&fixture.0, &executable).unwrap());
    let before = fixture.snapshot();
    run_with_git(&fixture.0, &options, &mut |_| panic!("--plan 不得启动 Git")).unwrap();
    assert_eq!(before, fixture.snapshot());
    assert!(!fixture.0.join(".local-tests").exists());
    assert!(!fixture.0.join("target").exists());
    for scope in [CheckScope::Backend, CheckScope::All, CheckScope::Frontend] {
        assert!(
            tasks_for(scope, &CheckPlanMode::ExplicitFull)
                .unwrap()
                .tasks
                .iter()
                .all(|task| !matches!(
                    task.executor,
                    TaskExecutor::LanguageReport
                        | TaskExecutor::LanguageWrite
                        | TaskExecutor::LanguageThreshold
                ))
        );
    }
}

#[test]
fn git_failure_malformed_paths_and_concurrent_file_changes_fail_closed() {
    let fixture = Fixture::new();
    let mut failure = |_: &[&str]| {
        Ok(git::Reply {
            code: 128,
            stdout: Vec::new(),
            stderr: b"not a git repository".to_vec(),
        })
    };
    assert!(
        run_with_git(&fixture.0, &LanguageOptions::default(), &mut failure)
            .unwrap_err()
            .to_string()
            .contains("Git 失败")
    );
    for invalid in [
        b"unterminated".as_slice(),
        b"\0",
        b"src.rs\0\0",
        b"x/./y.rs\0",
        b"../escape.rs\0",
        b"/absolute.rs\0",
        b"C:/outside.rs\0",
        b"dir\\file.rs\0",
        b"\xff\0",
    ] {
        assert!(git::paths(invalid).is_err());
    }
    fixture.write("src/lib.rs", b"old");
    let files = vec![policy::classify(&policy::load().unwrap(), "src/lib.rs", b"old").unwrap()];
    fixture.write("src/lib.rs", b"new");
    assert!(
        inventory::verify(&fixture.0, &files)
            .unwrap_err()
            .to_string()
            .contains("统计期间文件变化")
    );
    assert!(!fixture.0.join("target").exists());
}

#[test]
fn empty_git_tree_reports_no_ratio_and_explicit_threshold_fails() {
    let fixture = Fixture::new();
    fixture.git(&["init", "--quiet"]);
    let before = fixture.snapshot();
    let mut git = |args: &[&str]| git::invoke(&fixture.0, args);
    run_with_git(&fixture.0, &LanguageOptions::default(), &mut git).unwrap();
    let options = LanguageOptions {
        min_rust_percent: Some(Percent::parse("0").unwrap()),
        ..LanguageOptions::default()
    };
    let error = run_with_git(&fixture.0, &options, &mut git).unwrap_err();
    assert_eq!(failure_exit_code(error.as_ref()), Some(1), "{error}");
    assert_eq!(before, fixture.snapshot());
}

#[test]
fn explicit_create_only_report_preserves_failed_threshold_and_existing_outputs() {
    let fixture = Fixture::new();
    fixture.init_git();
    fixture.write("tools/check.py", b"print('below threshold')\n");
    fs::create_dir(fixture.0.join(".local-tests")).unwrap();
    let output = fixture.0.join(".local-tests/report.json");
    let options = LanguageOptions {
        min_rust_percent: Some(Percent::parse("80").unwrap()),
        output: Some(output.clone()),
        write: true,
        plan: false,
    };
    let mut git = |args: &[&str]| git::invoke(&fixture.0, args);
    let error = run_with_git(&fixture.0, &options, &mut git).unwrap_err();
    assert_eq!(failure_exit_code(error.as_ref()), Some(1), "{error}");
    let bytes = fs::read(&output).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["threshold_passed"], false);
    assert_eq!(value["rust_percent"], "0.000000");
    assert!(
        run_with_git(&fixture.0, &options, &mut git)
            .unwrap_err()
            .to_string()
            .contains("新文件")
    );
    assert_eq!(bytes, fs::read(&output).unwrap());
    assert!(!fixture.0.join("target").exists());
}

#[test]
fn cli_requires_explicit_threshold_write_pair_and_lexical_output_boundary() {
    assert_eq!(parsed(&[]).unwrap(), LanguageOptions::default());
    assert_eq!(
        parsed(&["--min-rust-percent", "80"])
            .unwrap()
            .min_rust_percent
            .unwrap()
            .text(),
        "80.000000"
    );
    assert!(
        parsed(&[
            "--output",
            ".local-tests/中文 空格.json",
            "--write",
            "--plan"
        ])
        .unwrap()
        .plan
    );
    for args in [
        vec!["--write"],
        vec!["--output", ".local-tests/report.json"],
        vec!["--output", "outside.json", "--write"],
        vec!["--output", ".local-tests/../outside.json", "--write"],
        vec!["--output", ".local-tests/report.txt", "--write"],
        vec!["--min-rust-percent", "80", "--min-rust-percent", "80"],
        vec![
            "--output",
            ".local-tests/a.json",
            "--output",
            ".local-tests/b.json",
            "--write",
        ],
        vec!["--plan", "--plan"],
        vec!["--write", "--write"],
        vec!["--unknown"],
    ] {
        assert!(parsed(&args).is_err(), "{args:?}");
    }
}

#[test]
fn every_git_inventory_phase_fails_closed_without_creating_output() {
    let fixture = Fixture::new();
    fixture.init_git();
    fixture.write("src/lib.rs", b"source");
    let before = fixture.snapshot();
    for command in [git::HEAD, git::FILES, git::IGNORED, git::DELETED] {
        let mut failing = |args: &[&str]| {
            if args == command {
                Ok(git::Reply {
                    code: 128,
                    stdout: Vec::new(),
                    stderr: b"injected Git failure".to_vec(),
                })
            } else {
                git::invoke(&fixture.0, args)
            }
        };
        assert!(
            run_with_git(&fixture.0, &LanguageOptions::default(), &mut failing).is_err(),
            "{command:?}"
        );
    }
    assert_eq!(before, fixture.snapshot());
}

#[test]
fn output_must_be_ignored_untracked_and_create_only_even_after_validation() {
    let fixture = Fixture::new();
    fixture.init_git();
    fixture.write("src/lib.rs", b"owned");
    fs::create_dir(fixture.0.join(".local-tests")).unwrap();
    let output = fixture.0.join(".local-tests/new.json");
    let mut git = |args: &[&str]| git::invoke(&fixture.0, args);
    report::validate_output(&fixture.0, &output, &mut git).unwrap();
    let inventory = inventory::discover(&fixture.0, &mut git).unwrap();
    let rules = policy::load().unwrap();
    let files = inventory::collect(&fixture.0, &rules, &inventory).unwrap();
    let report = report::build(rules, inventory, files, None).unwrap();
    fs::write(&output, b"concurrent owner").unwrap();
    assert!(report::write_new(&fixture.0, &output, &report).is_err());
    assert_eq!(fs::read(&output).unwrap(), b"concurrent owner");
    fixture.git(&["add", "-f", ".local-tests/new.json"]);
    fs::remove_file(&output).unwrap();
    assert!(
        report::validate_output(&fixture.0, &output, &mut git)
            .unwrap_err()
            .to_string()
            .contains("已跟踪")
    );
    fixture.write(".gitignore", "");
    let not_ignored = fixture.0.join(".local-tests/not-ignored.json");
    assert!(report::validate_output(&fixture.0, &not_ignored, &mut git).is_err());
    assert!(!not_ignored.exists());
}

#[test]
fn inventory_membership_changes_cannot_publish_a_mixed_report() {
    let fixture = Fixture::new();
    fixture.init_git();
    fixture.write("src/lib.rs", b"source");
    let mut calls = 0;
    let mut changed = |args: &[&str]| {
        if args == git::FILES {
            calls += 1;
            if calls == 2 {
                fixture.write("src/late.py", b"late change");
            }
        }
        git::invoke(&fixture.0, args)
    };
    let error = run_with_git(&fixture.0, &LanguageOptions::default(), &mut changed).unwrap_err();
    assert!(error.to_string().contains("Git 文件集合或 HEAD 变化"));
    assert!(!fixture.0.join("target").exists());
}

#[test]
fn every_registered_backend_generated_file_keeps_its_bytes_and_generated_marker() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let ownership: toml::Value = toml::from_str(
        &fs::read_to_string(root.join("catalog/resources/.ownership.toml")).unwrap(),
    )
    .unwrap();
    let rules = policy::load().unwrap();
    let mut rust = 0;
    for entry in ownership["entries"].as_array().unwrap() {
        if entry["root"].as_str() != Some("backend") {
            continue;
        }
        let path = entry["path"].as_str().unwrap();
        let bytes = fs::read(root.join(path)).unwrap();
        let file = policy::classify(&rules, path, &bytes).unwrap();
        assert!(file.generated, "受管文件未标记为生成：{path}");
        assert_eq!(file.bytes, bytes.len() as u64);
        if path.ends_with(".rs") {
            assert_eq!(file.kind, Kind::Code);
            assert_eq!(file.language.as_deref(), Some("Rust"));
            rust += 1;
        }
    }
    assert!(rust > 0);
}
