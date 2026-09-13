use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

use super::{
    cli::{CheckCommand, Command, RecoveryCommand, SeedSourceOperation, SeedSourceOptions, parse},
    recovery::seed_source_protocol_at,
    workspace::root_dir,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    request: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = root_dir().join(".local-tests").join(format!(
            "xtask-seed-source-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let request = directory.join("来源 请求.json");
        fs::write(&request, b"{\"secret\":\"must-not-enter-protocol\"}\n").unwrap();
        Self { directory, request }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn parse_source(
    fixture: &Fixture,
    operation: &str,
    request: bool,
    write: bool,
) -> Result<SeedSourceOptions, super::cli::CliError> {
    let mut arguments = vec![
        "check".to_owned(),
        "recovery".to_owned(),
        "clone".to_owned(),
        "seed-runtime".to_owned(),
        "--run-dir".to_owned(),
        fixture.directory.to_string_lossy().into_owned(),
        "--operation".to_owned(),
        operation.to_owned(),
    ];
    if request {
        arguments.extend([
            "--request".to_owned(),
            fixture.request.to_string_lossy().into_owned(),
        ]);
    }
    if write {
        arguments.push("--write".to_owned());
    }
    let command = parse(arguments)?.command;
    let Command::Check(CheckCommand::Recovery(RecoveryCommand::SeedSource(options))) = command
    else {
        panic!("正式 seed 来源操作必须解析为结构化请求");
    };
    Ok(options)
}

#[test]
fn parses_all_formal_source_operations_with_exact_shapes() {
    let fixture = Fixture::new();
    for (name, operation, request, write) in [
        (
            "source-register",
            SeedSourceOperation::Register,
            false,
            true,
        ),
        ("source-rebind", SeedSourceOperation::Rebind, true, true),
        (
            "source-generation-start",
            SeedSourceOperation::GenerationStart,
            true,
            true,
        ),
        (
            "source-generation-stop",
            SeedSourceOperation::GenerationStop,
            true,
            true,
        ),
        (
            "source-generation-status",
            SeedSourceOperation::GenerationStatus,
            false,
            false,
        ),
        (
            "source-generation-recover",
            SeedSourceOperation::GenerationRecover,
            true,
            true,
        ),
        ("source-export", SeedSourceOperation::Export, false, true),
        (
            "source-export-reconcile",
            SeedSourceOperation::ExportReconcile,
            false,
            true,
        ),
    ] {
        let parsed = parse_source(&fixture, name, request, write).unwrap();
        assert_eq!(parsed.operation, operation);
        assert_eq!(parsed.run_dir, fixture.directory);
        assert_eq!(parsed.request.as_ref(), request.then_some(&fixture.request));
        assert_eq!(parsed.write, write);
    }
}

#[test]
fn rejects_unknown_duplicate_cross_operation_and_invalid_write_arguments() {
    let fixture = Fixture::new();
    for (operation, request, write) in [
        ("source-rebind", false, true),
        ("source-register", true, true),
        ("source-export", false, false),
        ("source-generation-status", false, true),
    ] {
        assert!(parse_source(&fixture, operation, request, write).is_err());
    }

    let prefix = vec![
        "check".to_owned(),
        "recovery".to_owned(),
        "clone".to_owned(),
        "seed-runtime".to_owned(),
        "--run-dir".to_owned(),
        fixture.directory.to_string_lossy().into_owned(),
        "--operation".to_owned(),
        "source-register".to_owned(),
        "--write".to_owned(),
    ];
    for suffix in [
        vec!["--unknown".to_owned(), "value".to_owned()],
        vec!["--write".to_owned()],
        vec!["--operation".to_owned(), "source-export".to_owned()],
        vec![
            "--run-dir".to_owned(),
            fixture.directory.to_string_lossy().into_owned(),
        ],
        vec!["--request".to_owned()],
        vec![
            "--producer-binding".to_owned(),
            fixture.request.to_string_lossy().into_owned(),
        ],
    ] {
        assert!(parse([prefix.clone(), suffix].concat()).is_err());
    }
}

#[test]
fn rejects_paths_outside_local_tests_parent_jumps_missing_inputs_and_links() {
    let fixture = Fixture::new();
    let outside = root_dir().join("scripts");
    let parent_jump = fixture.directory.join("child/../request.json");
    let missing = fixture.directory.join("missing.json");
    for value in [outside, parent_jump] {
        let mut arguments = source_arguments(&fixture, "source-register", false, true);
        arguments[5] = value.to_string_lossy().into_owned();
        assert!(parse(arguments).is_err(), "路径应失败：{}", value.display());
    }
    let mut missing_request = source_arguments(&fixture, "source-rebind", true, true);
    missing_request[9] = missing.to_string_lossy().into_owned();
    assert!(parse(missing_request).is_err());

    let linked = fixture.directory.join("linked-run");
    create_directory_link(&fixture.directory.join("real-run"), &linked);
    let mut linked_arguments = source_arguments(&fixture, "source-register", false, true);
    linked_arguments[5] = linked.to_string_lossy().into_owned();
    assert!(parse(linked_arguments).is_err());
    fs::remove_dir(&linked).unwrap();
}

#[test]
fn private_protocol_has_exact_fields_paths_only_and_rechecks_inputs() {
    let fixture = Fixture::new();
    let options = parse_source(&fixture, "source-generation-start", true, true).unwrap();
    let payload = seed_source_protocol_at(&options, &root_dir()).unwrap();
    assert!(!payload.contains("must-not-enter-protocol"));
    assert!(!payload.contains(['\n', '\r', '\0']));
    let value: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(value["format_version"], 1);
    assert_eq!(value["kind"], "ryframe-xtask-recovery-seed-source");
    assert_eq!(value["request"]["operation"], "source-generation-start");
    assert_eq!(
        value["request"]["run_dir"],
        fixture.directory.to_str().unwrap()
    );
    assert_eq!(
        value["request"]["request"],
        fixture.request.to_str().unwrap()
    );
    assert_eq!(value["request"]["write"], true);
    assert_eq!(value["request"].as_object().unwrap().len(), 5);

    fs::remove_file(&fixture.request).unwrap();
    assert!(seed_source_protocol_at(&options, &root_dir()).is_err());
}

fn source_arguments(fixture: &Fixture, operation: &str, request: bool, write: bool) -> Vec<String> {
    let mut values = vec![
        "check".to_owned(),
        "recovery".to_owned(),
        "clone".to_owned(),
        "seed-runtime".to_owned(),
        "--run-dir".to_owned(),
        fixture.directory.to_string_lossy().into_owned(),
        "--operation".to_owned(),
        operation.to_owned(),
    ];
    if request {
        values.extend([
            "--request".to_owned(),
            fixture.request.to_string_lossy().into_owned(),
        ]);
    }
    if write {
        values.push("--write".to_owned());
    }
    values
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    fs::create_dir_all(target).unwrap();
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    fs::create_dir_all(target).unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .unwrap();
    assert!(status.success());
}
