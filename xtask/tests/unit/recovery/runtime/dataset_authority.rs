use super::*;

fn fixture() -> tempfile::TempDir {
    let base = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("dataset-authority-unit-")
        .tempdir_in(base)
        .unwrap()
}

#[test]
fn parallel_and_protected_serial_source_scans_preserve_complete_results_and_guards() {
    use crate::frontend_origin::{manifest, sources};
    use crate::recovery::clone::native::source;
    let directory = fixture();
    let root = directory.path();
    std::fs::create_dir(root.join("xtask")).unwrap();
    std::fs::write(root.join(sources::POLICY), br#"{"format_version":1,"files":["xtask/source-inventory.json"],"trees":["inputs"],"excluded_directories":[]}"#).unwrap();
    std::fs::create_dir(root.join("inputs")).unwrap();
    for index in 0..17 {
        std::fs::write(
            root.join(format!("inputs/来源-{index:02}.txt")),
            format!("input-{index}"),
        )
        .unwrap();
    }
    for arguments in [
        vec!["init", "--quiet"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=source-unit",
            "-c",
            "user.email=source-unit@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    ] {
        let mut command = std::process::Command::new("git");
        command.arg("-C").arg(root).args(arguments);
        for (name, _) in std::env::vars_os() {
            if name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
            {
                command.env_remove(name);
            }
        }
        assert!(
            command
                .env("GIT_OPTIONAL_LOCKS", "0")
                .status()
                .unwrap()
                .success()
        );
    }
    let parallel_source = source::capture(root).unwrap();
    let parallel_manifest = manifest::capture(root).unwrap();
    assert_eq!(parallel_source.files.len(), 18);
    assert_eq!(parallel_manifest.as_array().unwrap().len(), 18);
    let window = reads::Window::begin().unwrap();
    assert!(reads::is_active());
    assert_eq!(source::capture(root).unwrap(), parallel_source);
    assert_eq!(manifest::capture(root).unwrap(), parallel_manifest);
    // finish 会取出读取集合并再次采集完整来源；窗口仍须保持串行和完整保护。
    window.finish(Ok(())).unwrap();
    let changed = std::fs::write(root.join("inputs/来源-00.txt"), b"changed");
    let checked = window.finish(Ok(()));
    assert_eq!(checked.is_err(), changed.is_ok());
    drop(window);
    assert!(!reads::is_active());
    std::fs::write(
        root.join(sources::POLICY),
        br#"{"format_version":1,"files":["missing"],"trees":[],"excluded_directories":[]}"#,
    )
    .unwrap();
    assert!(manifest::capture(root).is_err());
    let window = reads::Window::begin().unwrap();
    assert!(manifest::capture(root).is_err());
    assert!(window.finish(Ok(())).is_ok());
}

#[test]
fn delegated_json_read_remains_protected_after_reader_returns() {
    let directory = fixture();
    let file = directory.path().join("input.json");
    std::fs::write(&file, b"{\"value\":1}").unwrap();
    let window = reads::Window::begin().unwrap();
    strict::read(&file).unwrap();
    let changed = std::fs::write(&file, b"{\"value\":2}");
    let result = window.finish(Ok(()));
    if changed.is_ok() {
        assert!(result.is_err());
    } else {
        assert!(result.is_ok());
    }
}

#[test]
fn optional_configuration_appearance_and_directory_additions_are_rejected() {
    let directory = fixture();
    let optional = directory.path().join("tools.toml");
    let window = reads::Window::begin().unwrap();
    reads::optional(&optional).unwrap();
    std::fs::write(&optional, b"[verification]").unwrap();
    assert!(window.finish(Ok(())).is_err());
    drop(window);
    let window = reads::Window::begin().unwrap();
    reads::directory(directory.path()).unwrap();
    std::fs::write(directory.path().join("unexpected.json"), b"{}").unwrap();
    assert!(window.finish(Ok(())).is_err());
}

#[test]
fn explicit_interpreter_is_required_without_falling_back_to_path() {
    let directory = fixture();
    let mut inputs = inputs::Inputs::new(directory.path());
    let error = commands::interpreter(directory.path(), &mut inputs)
        .unwrap_err()
        .to_string();
    assert!(error.contains("verification.python_program"));
}

#[test]
fn interpreter_drift_is_rejected_even_when_command_is_otherwise_unchanged() {
    let directory = fixture();
    let expected = directory.path().join("expected.exe");
    let different = directory.path().join("different.exe");
    std::fs::write(&expected, b"fixture-a").unwrap();
    std::fs::write(&different, b"fixture-b").unwrap();
    let expected = json!({"api":[expected,"--flag"],"worker":["unused"],"frontend":["unused"]});
    let mut actual = expected.clone();
    actual["api"][0] = json!(different);
    assert!(commands::recorded_spelling(expected, &actual).is_err());
}

#[test]
fn unknown_arguments_and_duplicate_fields_cannot_open_a_read_window() {
    assert!(
        request(&json!({"backend_dir":"x","runtime_receipt":"x","target_plan":"x","extra":true}))
            .is_err()
    );
    assert!(
        strict::parse_fragment(r#"{"backend_dir":"a","backend_dir":"b"}"#, "恢复参数").is_err()
    );
    let window = reads::Window::begin().unwrap();
    assert!(window.finish(Ok(())).is_ok());
}

#[test]
fn source_read_on_an_unbound_worker_thread_cannot_claim_complete_protection() {
    use crate::recovery::clone::native::source;
    let directory = fixture();
    let file = directory.path().join("input.json");
    std::fs::write(&file, b"{\"value\":1}").unwrap();
    let digest = digest::file(&file).unwrap();
    let inventory = source::Inventory {
        source: source::SourceBinding {
            snapshot: source::SourceSnapshot {
                head: "a".repeat(40),
                patch_sha256: "b".repeat(64),
                files: vec![],
                clean: true,
            },
            worktree_fingerprint: format!("sha256:{}", "c".repeat(64)),
        },
        files: vec![source::FileRecord {
            path: "input.json".into(),
            sha256: digest.sha256,
        }],
        guard: source::SourceGuard {
            head: "a".repeat(40),
            index_sha256: "d".repeat(64),
            modes_sha256: "e".repeat(64),
        },
    };
    let window = reads::Window::begin().unwrap();
    assert!(
        std::thread::spawn(move || strict::read(&file).is_ok())
            .join()
            .unwrap()
    );
    let error = reads::source(directory.path(), &inventory).unwrap_err();
    assert!(error.to_string().contains("并行来源读取"));
    assert!(window.finish::<()>(Err(error)).is_err());
}
