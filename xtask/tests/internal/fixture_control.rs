use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

use super::{
    cli::{
        CheckCommand, Command, FIXTURE_CONTROL_USAGE, FixtureArtifactCommand,
        FixtureControlCommand, FixtureEnvironmentCommand, FixtureRequestCommand,
        FixtureRetentionCommand, FixtureReviewCommand, FixtureServicesCommand,
        FixtureSourcePairCommand, FixtureSuccessorCommand, RecoveryCommand, parse,
    },
    recovery::fixture_control::private_invocation_at,
    workspace::root_dir,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    input: PathBuf,
    input_two: PathBuf,
    controlled_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = root_dir().join(".local-tests").join(format!(
            "xtask-fixture-control-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("input one.json");
        let input_two = directory.join("input two.json");
        fs::write(&input, b"{}\n").unwrap();
        fs::write(&input_two, b"{}\n").unwrap();
        let controlled_dir = directory.join("controlled directory");
        fs::create_dir(&controlled_dir).unwrap();
        Self {
            directory,
            input,
            input_two,
            controlled_dir,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }

    fn input_options(&self) -> Vec<String> {
        vec![
            "--review".to_owned(),
            text(&self.input),
            "--fixture".to_owned(),
            text(&self.input_two),
            "--maintenance-build".to_owned(),
            text(&self.input),
        ]
    }

    fn retention_options(&self) -> Vec<String> {
        vec![
            "--runtime-dir".to_owned(),
            text(&self.controlled_dir),
            "--tenant".to_owned(),
            "tenant-0123abcd".to_owned(),
            "--migration".to_owned(),
            "9223372036854775807".to_owned(),
        ]
    }

    fn generation_options(&self) -> Vec<String> {
        vec![
            "--successor".to_owned(),
            text(&self.input),
            "--source-backend".to_owned(),
            text(&root_dir()),
            "--expected-head".to_owned(),
            "a".repeat(40),
            "--backend-build".to_owned(),
            text(&self.input),
            "--maintenance-build".to_owned(),
            text(&self.input_two),
            "--source-environment".to_owned(),
            text(&self.input),
            "--id".to_owned(),
            "source-r24".to_owned(),
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

fn text(value: &Path) -> String {
    value.to_str().unwrap().to_owned()
}

fn words(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn parse_control(arguments: Vec<String>) -> Result<FixtureControlCommand, super::cli::CliError> {
    parse(arguments).map(|cli| match cli.command {
        Command::Check(CheckCommand::Recovery(RecoveryCommand::FixtureControl(command))) => {
            *command
        }
        _ => panic!("公开请求没有解析为 fixture control 类型"),
    })
}

fn command(domain: &str, operation: Option<&str>, values: Vec<String>) -> Vec<String> {
    let mut arguments = words(&["check", "recovery", "fixture", domain]);
    if let Some(operation) = operation {
        arguments.push(operation.to_owned());
    }
    arguments.extend(values);
    arguments
}

#[test]
fn parses_artifact_operations_and_serializes_job_id_as_text() {
    let fixture = Fixture::new();
    let receipt = fixture.path("artifact receipt.json");
    let snapshot = parse_control(command(
        "artifact",
        Some("snapshot"),
        vec![
            "--runtime-dir".to_owned(),
            text(&fixture.controlled_dir),
            "--job-id".to_owned(),
            "9223372036854775807".to_owned(),
            "--receipt".to_owned(),
            text(&receipt),
        ],
    ))
    .unwrap();
    assert!(matches!(
        snapshot,
        FixtureControlCommand::Artifact(FixtureArtifactCommand::Snapshot(_))
    ));
    let invocation = private_invocation_at(&snapshot, &root_dir()).unwrap();
    assert_eq!(invocation.script, "tools/python/full_stack_artifacts.py");
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(protocol["domain"], "artifact");
    assert_eq!(protocol["operation"], "snapshot");
    assert_eq!(protocol["runtime_dir"], text(&fixture.controlled_dir));
    assert_eq!(protocol["job_id"], "9223372036854775807");
    assert_eq!(protocol["receipt"], text(&receipt));
    assert_eq!(protocol["write"], true);

    fs::write(&receipt, b"{}\n").unwrap();
    let verify = parse_control(command(
        "artifact",
        Some("verify-deleted"),
        vec![
            "--runtime-dir".to_owned(),
            text(&fixture.controlled_dir),
            "--job-id".to_owned(),
            "1".to_owned(),
            "--receipt".to_owned(),
            text(&receipt),
        ],
    ))
    .unwrap();
    let invocation = private_invocation_at(&verify, &root_dir()).unwrap();
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(protocol["operation"], "verify-deleted");
    assert_eq!(protocol["write"], false);
}

#[test]
fn rejects_invalid_artifact_identifiers_writes_and_path_contracts() {
    let fixture = Fixture::new();
    let base = vec![
        "--runtime-dir".to_owned(),
        text(&fixture.controlled_dir),
        "--job-id".to_owned(),
        "123".to_owned(),
        "--receipt".to_owned(),
        text(&fixture.path("new receipt.json")),
    ];
    for job_id in ["0", "01", "-1", "9223372036854775808", "not-an-id"] {
        let mut values = base.clone();
        values[3] = job_id.to_owned();
        assert!(
            parse_control(command("artifact", Some("snapshot"), values)).is_err(),
            "{job_id}"
        );
    }
    for suffix in [
        vec!["--write".to_owned()],
        vec!["--job-id".to_owned(), "124".to_owned()],
        vec!["--unknown".to_owned(), "value".to_owned()],
    ] {
        let mut values = base.clone();
        values.extend(suffix);
        assert!(parse_control(command("artifact", Some("snapshot"), values)).is_err());
    }
    let mut relative = base.clone();
    relative[1] = "relative runtime".to_owned();
    assert!(parse_control(command("artifact", Some("snapshot"), relative)).is_err());
    let mut missing_receipt = base.clone();
    missing_receipt[5] = text(&fixture.path("missing receipt.json"));
    assert!(parse_control(command("artifact", Some("verify-deleted"), missing_receipt,)).is_err());
    let mut existing_snapshot_receipt = base;
    existing_snapshot_receipt[5] = text(&fixture.input);
    assert!(
        parse_control(command(
            "artifact",
            Some("snapshot"),
            existing_snapshot_receipt,
        ))
        .is_err()
    );
    assert!(parse_control(command("artifact", Some("snapshot"), Vec::new())).is_err());
}

#[test]
fn parses_retention_operations_and_serializes_private_values() {
    let fixture = Fixture::new();
    for (operation, expected_write) in [
        ("inspect", false),
        ("plan-history", false),
        ("verify-cleaned", false),
        ("export-backup", true),
    ] {
        let mut values = fixture.retention_options();
        if expected_write {
            values.push("--write".to_owned());
        }
        let command = parse_control(command("retention", Some(operation), values)).unwrap();
        assert!(matches!(
            command,
            FixtureControlCommand::Retention(FixtureRetentionCommand::Inspect(_))
                | FixtureControlCommand::Retention(FixtureRetentionCommand::PlanHistory(_))
                | FixtureControlCommand::Retention(FixtureRetentionCommand::VerifyCleaned(_))
                | FixtureControlCommand::Retention(FixtureRetentionCommand::ExportBackup(_))
        ));
        let invocation = private_invocation_at(&command, &root_dir()).unwrap();
        assert_eq!(
            invocation.script,
            "tools/python/full_stack_migration_history.py"
        );
        let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
        assert_eq!(protocol["domain"], "retention");
        assert_eq!(protocol["operation"], operation);
        assert_eq!(protocol["runtime_dir"], text(&fixture.controlled_dir));
        assert_eq!(protocol["tenant"], "tenant-0123abcd");
        assert_eq!(protocol["migration"], "9223372036854775807");
        assert_eq!(protocol["write"], expected_write);
        assert!(protocol.get("arguments").is_none());
    }

    let mut historical = fixture.retention_options();
    historical.extend([
        "--plan-sha256".to_owned(),
        "a".repeat(64),
        "--write".to_owned(),
    ]);
    let command =
        parse_control(command("retention", Some("historical-expired"), historical)).unwrap();
    assert!(matches!(
        command,
        FixtureControlCommand::Retention(FixtureRetentionCommand::HistoricalExpired { .. })
    ));
    let invocation = private_invocation_at(&command, &root_dir()).unwrap();
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(protocol["plan_sha256"], "a".repeat(64));
    assert_eq!(protocol["write"], true);
}

#[test]
fn retention_rejects_ambiguous_unsafe_or_incomplete_arguments() {
    let fixture = Fixture::new();
    let common = fixture.retention_options();
    let mut cases = Vec::new();
    cases.push(command(
        "retention",
        Some("historical-expired"),
        common.clone(),
    ));
    cases.push(command("retention", Some("export-backup"), common.clone()));
    let mut readonly_write = common.clone();
    readonly_write.push("--write".to_owned());
    cases.push(command("retention", Some("inspect"), readonly_write));
    let mut unrelated_sha = common.clone();
    unrelated_sha.extend(["--plan-sha256".to_owned(), "a".repeat(64)]);
    cases.push(command("retention", Some("plan-history"), unrelated_sha));
    for tenant in ["tenant-0123ABCd", "tenant-0123abc", "other-0123abcd"] {
        let mut values = common.clone();
        values[3] = tenant.to_owned();
        cases.push(command("retention", Some("inspect"), values));
    }
    for migration in ["0", "01", "-1", "9223372036854775808", "invalid"] {
        let mut values = common.clone();
        values[5] = migration.to_owned();
        cases.push(command("retention", Some("inspect"), values));
    }
    for digest in ["a".repeat(63), "A".repeat(64), "g".repeat(64)] {
        let mut values = common.clone();
        values.extend(["--plan-sha256".to_owned(), digest, "--write".to_owned()]);
        cases.push(command("retention", Some("historical-expired"), values));
    }
    let mut duplicate = common.clone();
    duplicate.extend(["--migration".to_owned(), "2".to_owned()]);
    cases.push(command("retention", Some("inspect"), duplicate));
    let mut unknown = common.clone();
    unknown.extend(["--unknown".to_owned(), "value".to_owned()]);
    cases.push(command("retention", Some("inspect"), unknown));
    let mut relative = common;
    relative[1] = "relative-runtime".to_owned();
    cases.push(command("retention", Some("inspect"), relative));
    cases.push(command(
        "retention",
        Some("destroy"),
        vec!["--help".to_owned()],
    ));
    cases.push(command("retention", Some("inspect"), Vec::new()));
    for arguments in cases {
        assert!(parse_control(arguments.clone()).is_err(), "{arguments:?}");
    }

    assert!(matches!(
        parse_control(command("retention", Some("--help"), Vec::new())).unwrap(),
        FixtureControlCommand::Retention(FixtureRetentionCommand::Help)
    ));
    assert!(matches!(
        parse_control(command(
            "retention",
            Some("historical-expired"),
            vec!["--help".to_owned()],
        ))
        .unwrap(),
        FixtureControlCommand::Retention(FixtureRetentionCommand::Help)
    ));
}

#[test]
fn parses_every_environment_operation_into_typed_requests() {
    let fixture = Fixture::new();
    let plan = parse_control(command(
        "environment",
        Some("plan"),
        fixture.input_options(),
    ))
    .unwrap();
    assert!(matches!(
        plan,
        FixtureControlCommand::Environment(FixtureEnvironmentCommand::Plan(_))
    ));

    let mut prepare = fixture.input_options();
    prepare.extend([
        "--output".to_owned(),
        text(&fixture.path("new environment")),
        "--secrets-dir".to_owned(),
        text(&fixture.controlled_dir),
        "--write".to_owned(),
    ]);
    assert!(matches!(
        parse_control(command("environment", Some("prepare"), prepare)).unwrap(),
        FixtureControlCommand::Environment(FixtureEnvironmentCommand::Prepare { .. })
    ));

    let cases = [
        (
            "review",
            vec![
                "--review".to_owned(),
                text(&fixture.input),
                "--output".to_owned(),
                text(&fixture.path("review.json")),
                "--write".to_owned(),
            ],
        ),
        (
            "rotate-secrets",
            vec![
                "--fixture".to_owned(),
                text(&fixture.input),
                "--output".to_owned(),
                text(&fixture.path("new secrets")),
                "--write".to_owned(),
            ],
        ),
        (
            "bootstrap-secrets",
            vec![
                "--source-fixture".to_owned(),
                text(&fixture.input),
                "--source-secrets".to_owned(),
                text(&fixture.controlled_dir),
                "--fixture".to_owned(),
                text(&fixture.input_two),
                "--write".to_owned(),
            ],
        ),
    ];
    for (operation, values) in cases {
        assert!(matches!(
            parse_control(command("environment", Some(operation), values)).unwrap(),
            FixtureControlCommand::Environment(_)
        ));
    }
}

#[test]
fn parses_review_request_and_all_service_operations() {
    let fixture = Fixture::new();
    let review = vec![
        "--template".to_owned(),
        text(&fixture.input),
        "--fixture".to_owned(),
        text(&fixture.input_two),
        "--future-root".to_owned(),
        text(&fixture.path("future root")),
        "--id".to_owned(),
        "r24-current".to_owned(),
        "--api-port".to_owned(),
        "18230".to_owned(),
        "--worker-port".to_owned(),
        "19230".to_owned(),
        "--frontend-port".to_owned(),
        "4200".to_owned(),
        "--rustfs-api-port".to_owned(),
        "29210".to_owned(),
        "--rustfs-console-port".to_owned(),
        "29211".to_owned(),
        "--redis-port".to_owned(),
        "16391".to_owned(),
        "--output".to_owned(),
        text(&fixture.path("renewed review.json")),
        "--write".to_owned(),
    ];
    assert!(matches!(
        parse_control(command("review", None, review)).unwrap(),
        FixtureControlCommand::Review(FixtureReviewCommand::Renew(_))
    ));

    let request = vec![
        "--environment".to_owned(),
        text(&fixture.input),
        "--service-run".to_owned(),
        text(&fixture.controlled_dir),
        "--id".to_owned(),
        "fresh-base".to_owned(),
        "--side".to_owned(),
        "base".to_owned(),
        "--output".to_owned(),
        text(&fixture.path("request.json")),
        "--write".to_owned(),
    ];
    assert!(matches!(
        parse_control(command("request", None, request)).unwrap(),
        FixtureControlCommand::Request(FixtureRequestCommand::Publish(_))
    ));

    for operation in [
        "rustfs",
        "redis",
        "buckets",
        "status",
        "close",
        "reconcile",
        "recover",
        "restart",
    ] {
        let mut values = vec![
            "--review".to_owned(),
            text(&fixture.input),
            "--environment".to_owned(),
            text(&fixture.input_two),
        ];
        if matches!(operation, "reconcile" | "recover" | "restart") {
            values.extend(["--owner-binding".to_owned(), text(&fixture.input)]);
        }
        if operation != "status" {
            values.push("--write".to_owned());
        }
        assert!(matches!(
            parse_control(command("services", Some(operation), values)).unwrap(),
            FixtureControlCommand::Services(FixtureServicesCommand::Run(_))
        ));
    }
}

#[test]
fn fixture_help_documents_explicit_service_reconciliation() {
    assert!(FIXTURE_CONTROL_USAGE.contains("services <reconcile|recover|restart>"));
}

#[test]
fn parses_successor_relationship() {
    let fixture = Fixture::new();
    let mut relationship = Vec::new();
    for name in [
        "--source-result",
        "--predecessor-review",
        "--predecessor-request",
        "--successor-review",
        "--seed-request",
        "--base-request",
        "--candidate-request",
    ] {
        relationship.extend([name.to_owned(), text(&fixture.input)]);
    }
    relationship.extend([
        "--id".to_owned(),
        "successor-r24".to_owned(),
        "--output".to_owned(),
        text(&fixture.path("relationship.json")),
        "--write".to_owned(),
    ]);
    assert!(matches!(
        parse_control(command("successor", Some("relationship"), relationship)).unwrap(),
        FixtureControlCommand::Successor(FixtureSuccessorCommand::Relationship(_))
    ));
}

#[test]
fn parses_successor_generation_preview_and_write_pairs() {
    let fixture = Fixture::new();
    let generation = fixture.generation_options();
    let planned = parse_control(command(
        "successor",
        Some("generation-request"),
        generation.clone(),
    ))
    .unwrap();
    assert!(matches!(
        planned,
        FixtureControlCommand::Successor(FixtureSuccessorCommand::GenerationRequest(ref value))
            if value.output.is_none()
    ));
    let invocation = private_invocation_at(&planned, &root_dir()).unwrap();
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(protocol["source_backend"], text(&root_dir()));
    assert_eq!(protocol["write"], false);
    let mut adapter = generation.clone();
    adapter.extend(words(&[
        "--adapter-contract",
        "legacy-stable-readiness-b0-v1",
        "--product-backend",
    ]));
    adapter.push(text(&root_dir()));
    let adapter = parse_control(command("successor", Some("generation-request"), adapter)).unwrap();
    let invocation = private_invocation_at(&adapter, &root_dir()).unwrap();
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(protocol["product_backend"], text(&root_dir()));
    let mut invalid_name = generation.clone();
    *invalid_name
        .iter_mut()
        .find(|value| value.as_str() == "source-r24")
        .unwrap() = "Source.R24".to_owned();
    assert!(
        parse_control(command(
            "successor",
            Some("generation-request"),
            invalid_name,
        ))
        .is_err()
    );
    let mut incomplete_adapter = generation.clone();
    incomplete_adapter.extend([
        "--adapter-contract".to_owned(),
        "legacy-stable-readiness-b0-v1".to_owned(),
    ]);
    assert!(
        parse_control(command(
            "successor",
            Some("generation-request"),
            incomplete_adapter,
        ))
        .is_err()
    );
    let mut unknown_adapter = generation.clone();
    unknown_adapter.extend([
        "--adapter-contract".to_owned(),
        "unregistered".to_owned(),
        "--product-backend".to_owned(),
        text(&fixture.controlled_dir),
    ]);
    assert!(
        parse_control(command(
            "successor",
            Some("generation-request"),
            unknown_adapter,
        ))
        .is_err()
    );
    let mut published = generation;
    published.extend([
        "--output".to_owned(),
        text(&fixture.path("generation.json")),
        "--write".to_owned(),
    ]);
    assert!(parse_control(command("successor", Some("generation-request"), published)).is_ok());
}

#[test]
fn successor_generation_keeps_source_and_evidence_path_boundaries() {
    let fixture = Fixture::new();
    let source = fixture.path("源码 directory");
    fs::create_dir(&source).unwrap();
    let link = fixture.path("linked source");
    create_directory_link(&source, &link);
    let mut valid = fixture.generation_options();
    valid[3] = text(&source);
    let parsed = parse_control(command(
        "successor",
        Some("generation-request"),
        valid.clone(),
    ))
    .unwrap();
    assert!(private_invocation_at(&parsed, &root_dir()).is_ok());
    for invalid in [
        PathBuf::from("relative"),
        fixture.input.clone(),
        fixture.path("missing"),
        link.clone(),
    ] {
        let mut arguments = valid.clone();
        arguments[3] = text(&invalid);
        assert!(
            parse_control(command("successor", Some("generation-request"), arguments)).is_err()
        );
        for product in [false, true] {
            let mut request = parsed.clone();
            let FixtureControlCommand::Successor(FixtureSuccessorCommand::GenerationRequest(
                options,
            )) = &mut request
            else {
                unreachable!();
            };
            if product {
                options.product_backend = Some(invalid.clone());
                options.adapter_contract = Some("legacy-stable-readiness-b0-v1".to_owned());
            } else {
                options.source_backend = invalid.clone();
            }
            assert!(private_invocation_at(&request, &root_dir()).is_err());
        }
    }
    #[cfg(unix)]
    fs::remove_file(&link).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&link).unwrap();
    for index in [1, 7, 9, 11] {
        let mut arguments = valid.clone();
        arguments[index] = text(&root_dir().join("Cargo.toml"));
        assert!(
            parse_control(command("successor", Some("generation-request"), arguments)).is_err()
        );
    }
    let mut outside_output = valid;
    outside_output.extend([
        "--output".to_owned(),
        text(&root_dir().join("forbidden-generation.json")),
        "--write".to_owned(),
    ]);
    assert!(
        parse_control(command(
            "successor",
            Some("generation-request"),
            outside_output
        ))
        .is_err()
    );
    assert!(!root_dir().join("forbidden-generation.json").exists());
}

#[test]
fn parses_successor_arm_request() {
    let fixture = Fixture::new();
    let arm = vec![
        "--successor".to_owned(),
        text(&fixture.input),
        "--source-export-result".to_owned(),
        text(&fixture.input_two),
        "--workspace".to_owned(),
        text(&fixture.controlled_dir),
        "--id".to_owned(),
        "arm-base-r24".to_owned(),
        "--side".to_owned(),
        "base".to_owned(),
        "--copy-directory".to_owned(),
        text(&fixture.path("copy base")),
    ];
    assert!(matches!(
        parse_control(command("successor", Some("arm-request"), arm)).unwrap(),
        FixtureControlCommand::Successor(FixtureSuccessorCommand::ArmRequest(_))
    ));
}

#[test]
fn source_pair_publish_is_typed_and_uses_the_fixed_private_script() {
    let fixture = Fixture::new();
    let output = fixture.path("source-pair.json");
    let command = parse_control(command(
        "source-pair",
        None,
        vec!["--output".to_owned(), text(&output), "--write".to_owned()],
    ))
    .unwrap();
    assert!(matches!(
        command,
        FixtureControlCommand::SourcePair(FixtureSourcePairCommand::Publish(_))
    ));
    let invocation = private_invocation_at(&command, &root_dir()).unwrap();
    assert_eq!(
        invocation.script,
        "tools/python/reference_fixture_source_pair.py"
    );
    let document: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(document["domain"], "source-pair");
    assert_eq!(document["operation"], "publish");
    assert_eq!(document["output"], text(&output));
    assert_eq!(document["write"], true);
}

#[test]
fn rejects_ambiguous_write_options_and_invalid_paths_before_dispatch() {
    let fixture = Fixture::new();
    let base = command(
        "services",
        Some("status"),
        vec![
            "--review".to_owned(),
            text(&fixture.input),
            "--environment".to_owned(),
            text(&fixture.input_two),
        ],
    );
    let mut cases = Vec::new();
    let mut write = base.clone();
    write.push("--write".to_owned());
    cases.push(write);
    let mut duplicate = base.clone();
    duplicate.extend(["--review".to_owned(), text(&fixture.input)]);
    cases.push(duplicate);
    let mut unknown = base.clone();
    unknown.extend(["--unknown".to_owned(), "value".to_owned()]);
    cases.push(unknown);
    let mut relative = base;
    relative[6] = "relative.json".to_owned();
    cases.push(relative);
    for arguments in cases {
        assert!(parse_control(arguments.clone()).is_err(), "{arguments:?}");
    }

    let missing_write = command(
        "environment",
        Some("rotate-secrets"),
        vec![
            "--fixture".to_owned(),
            text(&fixture.input),
            "--output".to_owned(),
            text(&fixture.path("secrets")),
        ],
    );
    assert!(parse_control(missing_write).is_err());
}

#[test]
fn private_protocol_selects_fixed_script_and_never_contains_argv_fields() {
    let fixture = Fixture::new();
    let command = parse_control(command(
        "services",
        Some("status"),
        vec![
            "--review".to_owned(),
            text(&fixture.input),
            "--environment".to_owned(),
            text(&fixture.input_two),
        ],
    ))
    .unwrap();
    let invocation = private_invocation_at(&command, &root_dir()).unwrap();
    assert_eq!(
        invocation.script,
        "tools/python/reference_fixture_services.py"
    );
    assert!(!invocation.protocol.contains(['\n', '\r', '\0']));
    let protocol: Value = serde_json::from_str(&invocation.protocol).unwrap();
    assert_eq!(protocol["format_version"], 1);
    assert_eq!(protocol["kind"], "ryframe-reference-fixture-control");
    assert_eq!(protocol["domain"], "services");
    assert_eq!(protocol["operation"], "status");
    assert_eq!(protocol["write"], false);
    assert_eq!(protocol["review"], text(&fixture.input));
    assert_eq!(protocol["environment"], text(&fixture.input_two));
    assert!(protocol.get("arguments").is_none());
}

#[test]
fn rejects_linked_components_before_private_protocol_creation() {
    let fixture = Fixture::new();
    let target = fixture.controlled_dir.join("linked input.json");
    fs::write(&target, b"{}\n").unwrap();
    let linked_directory = fixture.path("linked directory");
    create_directory_link(&fixture.controlled_dir, &linked_directory);
    let linked = linked_directory.join("linked input.json");
    let arguments = command(
        "services",
        Some("status"),
        vec![
            "--review".to_owned(),
            text(&linked),
            "--environment".to_owned(),
            text(&fixture.input_two),
        ],
    );
    assert!(parse_control(arguments).is_err());
    let artifact = command(
        "artifact",
        Some("snapshot"),
        vec![
            "--runtime-dir".to_owned(),
            text(&linked_directory),
            "--job-id".to_owned(),
            "1".to_owned(),
            "--receipt".to_owned(),
            text(&fixture.path("artifact receipt.json")),
        ],
    );
    assert!(parse_control(artifact).is_err());
    let retention = command(
        "retention",
        Some("inspect"),
        vec![
            "--runtime-dir".to_owned(),
            text(&linked_directory),
            "--tenant".to_owned(),
            "tenant-0123abcd".to_owned(),
            "--migration".to_owned(),
            "1".to_owned(),
        ],
    );
    assert!(parse_control(retention).is_err());
    let dataset = command(
        "dataset",
        Some("plan"),
        vec![
            "--environment".to_owned(),
            text(&fixture.input),
            "--runtime".to_owned(),
            text(&linked_directory),
            "--work-dir".to_owned(),
            text(&fixture.path("new dataset")),
            "--output".to_owned(),
            text(&fixture.path("dataset plan.json")),
            "--side".to_owned(),
            "base".to_owned(),
            "--write".to_owned(),
        ],
    );
    assert!(parse_control(dataset).is_err());
    #[cfg(unix)]
    fs::remove_file(linked_directory).unwrap();
    #[cfg(windows)]
    fs::remove_dir(linked_directory).unwrap();
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
