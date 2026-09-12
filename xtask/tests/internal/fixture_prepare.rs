use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

use super::{
    cli::{
        CheckCommand, Command, FixtureExpectedSources, FixturePrepareCommand,
        FixturePrepareOptions, RecoveryCommand, parse,
    },
    recovery::{fixture_prepare::private_invocation_at, recovery_command},
    workspace::{default_frontend_dir, root_dir},
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    output: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = root_dir().join(".local-tests").join(format!(
            "xtask-fixture-prepare-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let output = directory.join("Device 中文 fixture");
        Self { directory, output }
    }

    fn arguments(&self) -> Vec<String> {
        vec![
            "check".to_owned(),
            "recovery".to_owned(),
            "fixture".to_owned(),
            "--output-dir".to_owned(),
            text(&self.output),
            "--write".to_owned(),
            "--frontend-dir".to_owned(),
            text(&default_frontend_dir()),
        ]
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

fn parsed(arguments: Vec<String>) -> Result<FixturePrepareCommand, super::cli::CliError> {
    parse(arguments).map(|cli| match cli.command {
        Command::Check(CheckCommand::Recovery(RecoveryCommand::FixturePrepare(command))) => command,
        _ => panic!("Device 夹具准备没有解析为强类型请求"),
    })
}

#[test]
fn parses_development_and_formal_fixture_requests() {
    let fixture = Fixture::new();
    assert_eq!(
        parsed(fixture.arguments()).unwrap(),
        FixturePrepareCommand::Run(FixturePrepareOptions {
            output_dir: fixture.output.clone(),
            expected_sources: None,
        })
    );

    let mut formal = fixture.arguments();
    formal.extend([
        "--expected-backend-sha".to_owned(),
        "a".repeat(40),
        "--expected-frontend-sha".to_owned(),
        "b".repeat(40),
    ]);
    assert!(matches!(
        parsed(formal).unwrap(),
        FixturePrepareCommand::Run(FixturePrepareOptions {
            expected_sources: Some(FixtureExpectedSources { .. }),
            ..
        })
    ));
}

#[test]
fn rejects_invalid_or_ambiguous_requests_before_dispatch() {
    let fixture = Fixture::new();
    let valid = fixture.arguments();
    let mut missing_write = valid.clone();
    missing_write.retain(|value| value != "--write");
    let mut duplicate = valid.clone();
    duplicate.extend(["--output-dir".to_owned(), text(&fixture.output)]);
    let mut one_sha = valid.clone();
    one_sha.extend(["--expected-backend-sha".to_owned(), "a".repeat(40)]);
    let mut invalid_sha = valid.clone();
    invalid_sha.extend([
        "--expected-backend-sha".to_owned(),
        "A".repeat(40),
        "--expected-frontend-sha".to_owned(),
        "b".repeat(40),
    ]);
    for arguments in [missing_write, duplicate, one_sha, invalid_sha] {
        assert!(parsed(arguments).is_err());
    }

    fs::create_dir(&fixture.output).unwrap();
    assert!(parsed(valid).is_err());
}

#[test]
fn private_protocol_fixes_paths_and_hides_the_public_request_from_argv() {
    let fixture = Fixture::new();
    let options = FixturePrepareOptions {
        output_dir: fixture.output.clone(),
        expected_sources: Some(FixtureExpectedSources {
            backend_sha: "a".repeat(40),
            frontend_sha: "b".repeat(40),
        }),
    };
    let invocation = private_invocation_at(&options, &root_dir(), &default_frontend_dir()).unwrap();
    assert_eq!(invocation.script, "scripts/prepare_full_stack_fixture.py");
    let document: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(document["format_version"], 1);
    assert_eq!(document["kind"], "ryframe-reference-fixture-control");
    assert_eq!(document["domain"], "prepare");
    assert_eq!(document["operation"], "prepare");
    assert_eq!(document["output_dir"], text(&fixture.output));
    assert_eq!(document["expected_backend_sha"], "a".repeat(40));
    assert_eq!(document["expected_frontend_sha"], "b".repeat(40));
    assert!(document["write"].as_bool().unwrap());
    assert!(
        recovery_command(
            &RecoveryCommand::FixturePrepare(FixturePrepareCommand::Run(options)),
            &default_frontend_dir(),
        )
        .is_err()
    );
}
