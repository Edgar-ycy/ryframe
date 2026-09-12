use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{
    cli::{
        CacheOperation, CheckCommand, CloneCommand, CloneEffect, CloneRole, CloneRuntimeOperation,
        Command, PostCopyOperation, RecoveryCommand, SeedRuntimeOperation, StorageOperation, parse,
    },
    recovery::clone::protocol,
};

static NEXT: AtomicUsize = AtomicUsize::new(1);

struct Fixture {
    directory: PathBuf,
    run: PathBuf,
    first: PathBuf,
    second: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = super::workspace::root_dir().join(format!(
            ".local-tests/clone-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let run = directory.join("run");
        fs::create_dir_all(&run).unwrap();
        let first = directory.join("first.json");
        let second = directory.join("second.json");
        fs::write(&first, b"{}\n").unwrap();
        fs::write(&second, b"{}\n").unwrap();
        Self {
            directory,
            run,
            first,
            second,
        }
    }

    fn value(path: &Path) -> &str {
        path.to_str().unwrap()
    }

    fn parse(&self, values: &[&str]) -> Result<CloneCommand, String> {
        let arguments = ["check", "recovery", "clone"]
            .into_iter()
            .chain(values.iter().copied())
            .map(ToOwned::to_owned)
            .collect();
        let command = parse(arguments).map_err(|error| error.to_string())?.command;
        let Command::Check(CheckCommand::Recovery(RecoveryCommand::Clone(command))) = command
        else {
            return Err("没有解析为 clone 请求".into());
        };
        Ok(command)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.directory
                .starts_with(super::workspace::root_dir().join(".local-tests"))
        );
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn parses_core_clone_operations_and_enforces_write_policy() {
    let fixture = Fixture::new();
    let output = fixture.directory.join("plan.json");
    assert!(matches!(
        fixture.parse(&[
            "plan",
            "--input",
            Fixture::value(&fixture.first),
            "--output",
            Fixture::value(&output),
            "--write",
        ]),
        Ok(CloneCommand::Plan { .. })
    ));
    assert!(matches!(
        fixture.parse(&["verify", "--plan", Fixture::value(&fixture.first)]),
        Ok(CloneCommand::Verify { .. })
    ));
    assert!(matches!(
        fixture.parse(&["status", "--run-dir", Fixture::value(&fixture.run)]),
        Ok(CloneCommand::Status { .. })
    ));
    assert!(matches!(
        fixture.parse(&[
            "stage",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--stage",
            "copy",
            "--mode",
            "reconcile",
            "--write",
        ]),
        Ok(CloneCommand::Stage { .. })
    ));
    for invalid in [
        vec![
            "plan",
            "--input",
            Fixture::value(&fixture.first),
            "--output",
            Fixture::value(&output),
        ],
        vec![
            "verify",
            "--plan",
            Fixture::value(&fixture.first),
            "--write",
        ],
        vec!["status", "--run-dir", "../outside"],
        vec![
            "status",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--run-dir",
            Fixture::value(&fixture.run),
        ],
        vec![
            "stage",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--stage",
            "unknown",
            "--write",
        ],
        vec![
            "stage",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--stage",
            "target-verify",
            "--mode",
            "resume",
            "--write",
        ],
    ] {
        assert!(fixture.parse(&invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn parses_service_controls_without_ambiguous_roles_or_requests() {
    let fixture = Fixture::new();
    let runtime = fixture
        .parse(&[
            "runtime",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--side",
            "source",
            "--operation",
            "status",
            "--roles",
            "api",
        ])
        .unwrap();
    let CloneCommand::Runtime(runtime) = runtime else {
        panic!("runtime 类型错误");
    };
    assert_eq!(runtime.operation, CloneRuntimeOperation::Status);
    assert_eq!(runtime.roles, vec![CloneRole::Api]);

    let storage = fixture
        .parse(&[
            "storage",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--side",
            "target",
            "--operation",
            "restart",
            "--request",
            Fixture::value(&fixture.first),
            "--write",
        ])
        .unwrap();
    assert!(
        matches!(storage, CloneCommand::Storage(value) if value.operation == StorageOperation::Restart)
    );
    let cache = fixture
        .parse(&[
            "cache",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--operation",
            "status",
        ])
        .unwrap();
    assert!(
        matches!(cache, CloneCommand::Cache(value) if value.operation == CacheOperation::Status)
    );

    for invalid in [
        vec![
            "runtime",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--side",
            "source",
            "--operation",
            "status",
            "--write",
        ],
        vec![
            "runtime",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--side",
            "source",
            "--operation",
            "start",
            "--roles",
            "api",
            "api",
            "--write",
        ],
        vec![
            "storage",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--side",
            "target",
            "--operation",
            "stop",
            "--request",
            Fixture::value(&fixture.first),
            "--write",
        ],
        vec![
            "cache",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--operation",
            "restart",
        ],
    ] {
        assert!(fixture.parse(&invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn parses_post_copy_and_non_source_seed_operations_with_exact_inputs() {
    let fixture = Fixture::new();
    let post = fixture
        .parse(&[
            "post-copy",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--operation",
            "register",
            "--request",
            Fixture::value(&fixture.first),
            "--write",
        ])
        .unwrap();
    assert!(
        matches!(post, CloneCommand::PostCopy(value) if value.operation == PostCopyOperation::Register)
    );
    let seed = fixture
        .parse(&[
            "seed-runtime",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--operation",
            "arm-input",
            "--request",
            Fixture::value(&fixture.second),
            "--write",
        ])
        .unwrap();
    assert!(
        matches!(seed, CloneCommand::SeedRuntime(value) if value.operation == SeedRuntimeOperation::ArmInput)
    );
    assert!(matches!(
        fixture.parse(&[
            "seed-runtime", "--run-dir", Fixture::value(&fixture.run), "--operation", "status",
        ]),
        Ok(CloneCommand::SeedRuntime(value)) if value.operation == SeedRuntimeOperation::Status
    ));
    for invalid in [
        vec![
            "post-copy",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--operation",
            "prepare",
            "--request",
            Fixture::value(&fixture.first),
            "--write",
        ],
        vec![
            "post-copy",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--operation",
            "recover-session",
            "--write",
        ],
        vec![
            "seed-runtime",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--operation",
            "start",
            "--request",
            Fixture::value(&fixture.first),
            "--write",
        ],
        vec![
            "seed-runtime",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--operation",
            "status",
            "--write",
        ],
    ] {
        assert!(fixture.parse(&invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn classifies_observation_and_resource_mutations_precisely() {
    let fixture = Fixture::new();
    let cases = [
        (
            vec![
                "stage",
                "--run-dir",
                Fixture::value(&fixture.run),
                "--stage",
                "copy",
                "--mode",
                "reconcile",
                "--write",
            ],
            CloneEffect::EvidenceWrite,
        ),
        (
            vec![
                "runtime",
                "--run-dir",
                Fixture::value(&fixture.run),
                "--side",
                "target",
                "--operation",
                "recover",
                "--write",
            ],
            CloneEffect::EvidenceWrite,
        ),
        (
            vec![
                "post-copy",
                "--run-dir",
                Fixture::value(&fixture.run),
                "--operation",
                "reconcile",
                "--write",
            ],
            CloneEffect::EvidenceWrite,
        ),
        (
            vec![
                "seed-runtime",
                "--run-dir",
                Fixture::value(&fixture.run),
                "--operation",
                "quotas-reconcile",
                "--write",
            ],
            CloneEffect::EvidenceWrite,
        ),
        (
            vec![
                "cache",
                "--run-dir",
                Fixture::value(&fixture.run),
                "--operation",
                "resume",
                "--write",
            ],
            CloneEffect::BusinessWrite,
        ),
    ];
    for (arguments, expected) in cases {
        assert_eq!(fixture.parse(&arguments).unwrap().effect(), expected);
    }
}

#[test]
fn private_protocol_binds_exact_paths_and_rechecks_file_identity() {
    let fixture = Fixture::new();
    let command = fixture
        .parse(&[
            "bridge",
            "--build",
            Fixture::value(&fixture.first),
            "--inventory",
            Fixture::value(&fixture.second),
            "--output",
            Fixture::value(&fixture.directory.join("bridge.json")),
            "--write",
        ])
        .unwrap();
    let payload = protocol(&command, &super::workspace::root_dir()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(value["format_version"], 1);
    assert_eq!(value["kind"], "ryframe-xtask-recovery-clone");
    assert_eq!(value["request"]["command"], "bridge");
    assert_eq!(value["request"]["effect"], "evidence-write");
    assert_eq!(value["request"]["write"], true);
    assert_eq!(
        PathBuf::from(value["request"]["options"]["build"].as_str().unwrap()),
        fixture.first
    );

    fs::remove_file(&fixture.first).unwrap();
    assert!(protocol(&command, &super::workspace::root_dir()).is_err());
}
