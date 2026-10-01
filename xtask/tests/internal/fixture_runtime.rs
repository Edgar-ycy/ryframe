use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

use super::{
    cli::{CheckCommand, Command, FixtureRuntimeCommand, FixtureServer, RecoveryCommand, parse},
    recovery::fixture_runtime::private_invocation_at,
    workspace::root_dir,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    environment: PathBuf,
    runtime: PathBuf,
    binding: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = root_dir().join(".local-tests").join(format!(
            "xtask-fixture-runtime-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let environment = directory.join("bootstrap 环境.json");
        fs::write(&environment, b"{}\n").unwrap();
        let runtime = directory.join("runtime-r1");
        fs::create_dir(&runtime).unwrap();
        let binding = runtime.join("browser-binding-r24-device.json");
        fs::write(&binding, b"{}\n").unwrap();
        Self {
            directory,
            environment,
            runtime,
            binding,
        }
    }

    fn arguments(&self, operation: &str) -> Vec<String> {
        [
            strings(&["check", "recovery", "fixture", "runtime", operation]),
            vec![
                "--environment".to_owned(),
                text(&self.environment),
                "--output".to_owned(),
                text(&self.runtime),
            ],
        ]
        .concat()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn text(value: &Path) -> String {
    value.to_str().unwrap().to_owned()
}

fn parse_runtime(arguments: Vec<String>) -> Result<FixtureRuntimeCommand, super::cli::CliError> {
    parse(arguments).map(|cli| match cli.command {
        Command::Check(CheckCommand::Recovery(RecoveryCommand::FixtureRuntime(command))) => command,
        _ => panic!("公开请求没有解析为 fixture runtime 类型"),
    })
}

#[test]
fn parses_all_runtime_operations_into_three_typed_shapes() {
    let fixture = Fixture::new();
    let build_output = fixture.directory.join("new runtime");
    let build = parse_runtime(
        [
            strings(&["check", "recovery", "fixture", "runtime", "build"]),
            vec![
                "--environment".to_owned(),
                text(&fixture.environment),
                "--output".to_owned(),
                text(&build_output),
                "--write".to_owned(),
            ],
        ]
        .concat(),
    )
    .unwrap();
    assert!(matches!(build, FixtureRuntimeCommand::Build(_)));

    for operation in ["verify", "status"] {
        assert!(matches!(
            parse_runtime(fixture.arguments(operation)).unwrap(),
            FixtureRuntimeCommand::Verify(_) | FixtureRuntimeCommand::Status(_)
        ));
    }
    for operation in ["start", "stop"] {
        let mut arguments = fixture.arguments(operation);
        arguments.push("--write".to_owned());
        assert!(matches!(
            parse_runtime(arguments).unwrap(),
            FixtureRuntimeCommand::Start(_) | FixtureRuntimeCommand::Stop(_)
        ));
    }
    for (operation, expected) in [
        ("browser", "browser"),
        ("browser-verify", "browser-verify"),
        ("browser-close", "browser-close"),
    ] {
        let mut arguments = fixture.arguments(operation);
        arguments.extend(["--browser-binding".to_owned(), text(&fixture.binding)]);
        if operation == "browser" {
            arguments.push("--write".to_owned());
        }
        let command = parse_runtime(arguments).unwrap();
        assert_eq!(command.operation(), expected);
        assert_eq!(command.browser_binding(), Some(&fixture.binding));
    }
}

#[test]
fn parses_bind_with_new_exactly_named_binding() {
    let fixture = Fixture::new();
    let binding = fixture.runtime.join("browser-binding-r25-device.json");
    let mut arguments = fixture.arguments("bind");
    arguments.extend(strings(&[
        "--browser-binding",
        binding.to_str().unwrap(),
        "--run-id",
        "r25-device",
        "--server",
        "preview",
        "--write",
    ]));
    let command = parse_runtime(arguments).unwrap();
    let invocation = private_invocation_at(&command, &root_dir()).unwrap();
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(protocol["browser_binding"], text(&binding));
    assert_eq!(protocol["run_id"], "r25-device");
    assert_eq!(protocol["server"], "preview");
    assert_eq!(protocol["write"], true);
    assert_eq!(
        command,
        FixtureRuntimeCommand::Bind {
            binding: super::cli::FixtureBrowserBinding {
                runtime: super::cli::FixtureRuntimePaths {
                    environment: fixture.environment.clone(),
                    output: fixture.runtime.clone(),
                },
                browser_binding: binding,
            },
            run_id: "r25-device".to_owned(),
            server: FixtureServer::Preview,
        }
    );
}

#[test]
fn rejects_ambiguous_write_and_option_combinations_before_dispatch() {
    let fixture = Fixture::new();
    let mut cases = Vec::new();
    let mut missing_write = fixture.arguments("start");
    cases.push(missing_write.clone());
    missing_write.extend(strings(&["--write", "--write"]));
    cases.push(missing_write);
    let mut read_write = fixture.arguments("verify");
    read_write.push("--write".to_owned());
    cases.push(read_write);
    let mut unknown = fixture.arguments("verify");
    unknown.extend(strings(&["--unknown", "value"]));
    cases.push(unknown);
    let mut duplicate = fixture.arguments("verify");
    duplicate.extend(strings(&["--output", fixture.runtime.to_str().unwrap()]));
    cases.push(duplicate);
    let mut plain_browser_field = fixture.arguments("verify");
    plain_browser_field.extend(strings(&["--run-id", "r24-device"]));
    cases.push(plain_browser_field);
    for arguments in cases {
        assert!(parse_runtime(arguments.clone()).is_err(), "{arguments:?}");
    }
}

#[test]
fn rejects_paths_outside_the_current_backend_or_with_wrong_state() {
    let fixture = Fixture::new();
    let mut cases = Vec::new();
    let mut relative = fixture.arguments("verify");
    relative[6] = "relative.json".to_owned();
    cases.push(relative);
    let mut outside = fixture.arguments("verify");
    outside[8] = text(&root_dir());
    cases.push(outside);
    let mut missing_environment = fixture.arguments("verify");
    missing_environment[6] = text(&fixture.directory.join("missing.json"));
    cases.push(missing_environment);
    let mut missing_runtime = fixture.arguments("verify");
    missing_runtime[8] = text(&fixture.directory.join("missing-runtime"));
    cases.push(missing_runtime);
    let mut existing_build = fixture.arguments("build");
    existing_build.push("--write".to_owned());
    cases.push(existing_build);
    for arguments in cases {
        assert!(parse_runtime(arguments.clone()).is_err(), "{arguments:?}");
    }
}

#[test]
fn rejects_browser_binding_drift_before_dispatch() {
    let fixture = Fixture::new();
    let other = fixture.directory.join("other");
    fs::create_dir(&other).unwrap();
    let outside_binding = other.join("browser-binding-r24-device.json");
    fs::write(&outside_binding, b"{}\n").unwrap();
    let mut wrong_parent = fixture.arguments("browser-verify");
    wrong_parent.extend(["--browser-binding".to_owned(), text(&outside_binding)]);

    let mut invalid_server = fixture.arguments("bind");
    invalid_server.extend(strings(&[
        "--browser-binding",
        fixture
            .runtime
            .join("browser-binding-r25.json")
            .to_str()
            .unwrap(),
        "--run-id",
        "r25",
        "--server",
        "test",
        "--write",
    ]));
    let mut invalid_run = fixture.arguments("bind");
    invalid_run.extend(strings(&[
        "--browser-binding",
        fixture
            .runtime
            .join("browser-binding-BAD.json")
            .to_str()
            .unwrap(),
        "--run-id",
        "BAD",
        "--server",
        "dev",
        "--write",
    ]));
    for arguments in [wrong_parent, invalid_server, invalid_run] {
        assert!(parse_runtime(arguments.clone()).is_err(), "{arguments:?}");
    }
}

#[test]
fn private_invocation_has_no_path_arguments_and_an_exact_read_only_protocol() {
    let fixture = Fixture::new();
    let command = parse_runtime(fixture.arguments("verify")).unwrap();
    let invocation = private_invocation_at(&command, &root_dir()).unwrap();
    assert_eq!(
        invocation.script,
        "tools/python/reference_fixture_runtime.py"
    );
    assert!(!invocation.protocol.contains(['\n', '\r', '\0']));
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(
        protocol.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "backend_dir",
            "environment",
            "format_version",
            "operation",
            "output",
            "write"
        ]
    );
    assert_eq!(protocol["operation"], "verify");
    assert_eq!(protocol["write"], false);
    assert_eq!(protocol["environment"], text(&fixture.environment));
    assert_eq!(protocol["output"], text(&fixture.runtime));
}

#[test]
fn rejects_reparse_or_link_components_before_protocol_creation() {
    let fixture = Fixture::new();
    let linked = fixture.directory.join("linked-runtime");
    create_directory_link(&fixture.runtime, &linked);
    let mut arguments = fixture.arguments("verify");
    arguments[8] = text(&linked);
    assert!(parse_runtime(arguments).is_err());
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
