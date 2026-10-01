use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

use super::{
    cli::{Command, DataCommand, PerformanceIdentitiesCommand, parse},
    data::performance_identities::private_invocation_at,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    workspace::root_dir,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = root_dir().join(".local-tests").join(format!(
            "xtask-performance-identities-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }

    fn file(&self, name: &str) -> PathBuf {
        let target = self.directory.join(name);
        fs::write(&target, b"{}\n").unwrap();
        target
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn parse_command(arguments: &[String]) -> Result<Command, super::cli::CliError> {
    parse(arguments.to_vec()).map(|cli| cli.command)
}

fn arguments(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn parses_three_explicit_write_operations_with_absolute_local_paths() {
    let fixture = Fixture::new();
    let environment = fixture.file("性能 environment.json");
    let output = fixture.directory.join("identity plan.json");
    let plan = fixture.file("existing plan.json");
    let state_dir = fixture.directory.join("identity state");
    let command = parse_command(&arguments(&[
        "data",
        "performance-identities",
        "plan",
        "--environment",
        environment.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--write",
    ]))
    .unwrap();
    assert_eq!(
        command,
        Command::Data(DataCommand::PerformanceIdentities(
            PerformanceIdentitiesCommand::Plan {
                environment,
                output,
            }
        ))
    );
    for operation in ["apply", "verify"] {
        let command = parse_command(&arguments(&[
            "data",
            "performance-identities",
            operation,
            "--plan",
            plan.to_str().unwrap(),
            "--state-dir",
            state_dir.to_str().unwrap(),
            "--write",
        ]))
        .unwrap();
        assert!(matches!(
            command,
            Command::Data(DataCommand::PerformanceIdentities(
                PerformanceIdentitiesCommand::Apply { .. }
                    | PerformanceIdentitiesCommand::Verify { .. }
            ))
        ));
    }
}

#[test]
fn rejects_ambiguous_options_and_every_non_local_path_before_dispatch() {
    let fixture = Fixture::new();
    let environment = fixture.file("environment.json");
    let output = fixture.directory.join("plan.json");
    let outside = root_dir().join("README.md");
    let parent_jump = fixture.directory.join("child/../plan.json");
    let existing_output = fixture.file("existing-output.json");
    let file_parent = fixture.file("file-parent").join("plan.json");
    let valid = [
        "data".to_owned(),
        "performance-identities".to_owned(),
        "plan".to_owned(),
        "--environment".to_owned(),
        environment.to_string_lossy().into_owned(),
        "--output".to_owned(),
        output.to_string_lossy().into_owned(),
        "--write".to_owned(),
    ];
    let mut cases = vec![valid[..valid.len() - 1].to_vec()];
    cases.push([valid.as_slice(), &["--write".to_owned()]].concat());
    cases.push([valid.as_slice(), &["--unknown".to_owned()]].concat());
    for invalid_output in [
        Path::new("relative.json"),
        outside.as_path(),
        parent_jump.as_path(),
        existing_output.as_path(),
        file_parent.as_path(),
    ] {
        let mut case = valid.clone();
        case[6] = invalid_output.to_string_lossy().into_owned();
        cases.push(case.to_vec());
    }
    let mut cross_operation = valid;
    cross_operation[2] = "apply".to_owned();
    cases.push(cross_operation.to_vec());
    for case in cases {
        assert!(parse_command(&case).is_err(), "参数应失败：{case:?}");
    }
}

#[test]
fn private_invocation_has_a_fixed_script_no_arguments_and_exact_json() {
    let fixture = Fixture::new();
    let environment = fixture.file("环境 清单.json");
    let output = fixture.directory.join("身份 计划.json");
    let command = PerformanceIdentitiesCommand::Plan {
        environment: environment.clone(),
        output: output.clone(),
    };
    let invocation = private_invocation_at(&command, &root_dir()).unwrap();
    assert_eq!(invocation.script, "tools/js/devex_prepare_identities.mjs");
    assert!(!invocation.protocol.contains(['\n', '\r', '\0']));
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(
        protocol.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "environment",
            "format_version",
            "operation",
            "output",
            "write"
        ]
    );
    assert_eq!(protocol["format_version"], 1);
    assert_eq!(protocol["operation"], "plan");
    assert_eq!(protocol["environment"], environment.to_str().unwrap());
    assert_eq!(protocol["output"], output.to_str().unwrap());
    assert_eq!(protocol["write"], true);
}

#[test]
fn rejects_link_or_junction_components_even_when_the_target_exists() {
    let fixture = Fixture::new();
    let linked = fixture.directory.join("linked-scripts");
    create_directory_link(&root_dir().join("scripts"), &linked);
    let target = linked.join("devex_prepare_identities.mjs");
    let error = validate_local_test_path(&target, &root_dir(), LocalTestPathKind::ExistingFile)
        .unwrap_err();
    assert!(
        error.contains("链接") || error.contains("junction"),
        "{error}"
    );
    #[cfg(unix)]
    fs::remove_file(&linked).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&linked).unwrap();
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .unwrap();
    assert!(status.success());
}
