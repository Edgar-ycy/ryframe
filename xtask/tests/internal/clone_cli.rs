use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::cli::{
    CacheOperation, CheckCommand, CloneCommand, CloneEffect, CloneRole, CloneRuntimeOperation,
    Command, PostCopyOperation, RecoveryCommand, SeedRuntimeOperation, StorageOperation, parse,
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
fn copy_lock_recovery_requires_explicit_write() {
    let fixture = Fixture::new();
    let args = [
        "recover-copy",
        "--run-dir",
        Fixture::value(&fixture.run),
        "--owner-binding",
        Fixture::value(&fixture.first),
    ];
    assert!(fixture.parse(&args).is_err());
    let mut authorized = args.to_vec();
    authorized.push("--write");
    let command = fixture.parse(&authorized).unwrap();
    assert!(matches!(command, CloneCommand::RecoverCopy { .. }));
}

#[test]
fn only_ordinary_recovery_accepts_its_removed_owner_path() {
    let fixture = Fixture::new();
    let removed = fixture.run.join("run.lock/owner.json");
    let other = fixture.run.join("controller-0001.json");
    assert!(!removed.parent().unwrap().exists());
    let mut args = [
        "recover",
        "--run-dir",
        Fixture::value(&fixture.run),
        "--owner-binding",
        Fixture::value(&removed),
        "--write",
    ];
    assert!(fixture.parse(&args[..5]).is_err());
    assert!(matches!(
        fixture.parse(&args),
        Ok(CloneCommand::Recover { run_dir, owner_binding })
            if run_dir == fixture.run && owner_binding == removed
    ));
    args[0] = "recover-copy";
    assert!(fixture.parse(&args).is_err());
    args[0] = "recover";
    args[4] = Fixture::value(&other);
    assert!(fixture.parse(&args).is_err());
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
fn api_successor_requires_target_api_and_explicit_write() {
    let fixture = Fixture::new();
    for (name, operation, effect) in [
        (
            "restart-api",
            CloneRuntimeOperation::RestartApi,
            CloneEffect::BusinessWrite,
        ),
        (
            "reconcile-api",
            CloneRuntimeOperation::ReconcileApi,
            CloneEffect::EvidenceWrite,
        ),
    ] {
        let parse = |side, extra: &[&str]| {
            let mut args = vec![
                "runtime",
                "--run-dir",
                Fixture::value(&fixture.run),
                "--side",
                side,
                "--operation",
                name,
            ];
            args.extend_from_slice(extra);
            fixture.parse(&args)
        };
        assert!(parse("target", &[]).is_err());
        let CloneCommand::Runtime(parsed) = parse("target", &["--write"]).unwrap() else {
            panic!("runtime 类型错误");
        };
        assert_eq!(parsed.operation, operation);
        assert_eq!(parsed.operation.as_str(), name);
        assert_eq!(parsed.operation.effect(), effect);
        assert_eq!(parsed.roles, [CloneRole::Api]);
        assert_eq!(parsed.request, None);

        assert!(parse("target", &["--write", "--roles", "api"]).is_ok());
        assert!(parse("target", &["--write", "--roles", "api", "worker"]).is_err());
        assert!(parse("source", &["--write"]).is_err());
        let CloneCommand::Runtime(requested) = parse(
            "target",
            &["--write", "--request", Fixture::value(&fixture.first)],
        )
        .unwrap() else {
            panic!("runtime 类型错误");
        };
        assert_eq!(requested.request.as_deref(), Some(fixture.first.as_path()));
    }
    for operation in ["start", "stop", "status", "recover"] {
        let mut args = vec![
            "runtime",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--side",
            "target",
            "--operation",
            operation,
            "--request",
            Fixture::value(&fixture.first),
        ];
        if operation != "status" {
            args.push("--write");
        }
        assert!(fixture.parse(&args).is_err());
    }
}

#[test]
fn post_copy_held_originals_require_supported_windows_operations() {
    let fixture = Fixture::new();
    let run = Fixture::value(&fixture.run);
    let descriptor = Fixture::value(&fixture.first);
    for (mode, option, supported) in [
        ("prepare", None, true),
        ("schedules", None, true),
        ("verify", None, true),
        ("reconcile", None, false),
        ("register", Some("--request"), false),
        ("amend", Some("--request"), false),
        ("recover-session", Some("--producer-binding"), false),
    ] {
        let mut args = vec![
            "post-copy",
            "--run-dir",
            run,
            "--operation",
            mode,
            "--write",
        ];
        if let Some(option) = option {
            args.extend([option, descriptor]);
        }
        assert!(
            matches!(
                fixture.parse(&args),
                Ok(CloneCommand::PostCopy(options)) if !options.hold_originals
            ),
            "{mode}"
        );
        args.extend(["--originals", "held"]);
        let held = fixture.parse(&args);
        if cfg!(windows) && supported {
            assert!(
                matches!(
                    held,
                    Ok(CloneCommand::PostCopy(options)) if options.hold_originals
                ),
                "{mode}"
            );
        } else {
            assert!(held.is_err(), "{mode}");
        }
    }
}

#[test]
fn prepare_lock_recovery_requires_explicit_bound_operations() {
    let fixture = Fixture::new();
    let run = Fixture::value(&fixture.run);
    let owner = Fixture::value(&fixture.first);
    let output = fixture.directory.join("prepare-lock.json");
    for (operation, expected) in [
        ("prepare-lock-observe", false),
        ("prepare-lock-observe-interrupted", true),
    ] {
        let observe = [
            operation,
            "--run-dir",
            run,
            "--owner-binding",
            owner,
            "--output",
            Fixture::value(&output),
            "--write",
        ];
        assert!(matches!(
            fixture.parse(&observe),
            Ok(CloneCommand::ObservePrepareTargetLock { output: path, after_exit, .. })
                if path == output && after_exit == expected
        ));
        assert!(fixture.parse(&observe[..observe.len() - 1]).is_err());
    }
    let recover = [
        "prepare-lock-recover",
        "--run-dir",
        run,
        "--request",
        owner,
        "--write",
    ];
    assert!(matches!(
        fixture.parse(&recover),
        Ok(CloneCommand::RecoverPrepareTargetLock { receipt, .. }) if receipt == fixture.first
    ));
    assert!(fixture.parse(&recover[..recover.len() - 1]).is_err());
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
        matches!(&seed, CloneCommand::SeedRuntime(value) if value.operation == SeedRuntimeOperation::ArmInput)
    );
    assert!(matches!(
        fixture.parse(&[
            "seed-runtime", "--run-dir", Fixture::value(&fixture.run), "--operation", "status",
        ]),
        Ok(CloneCommand::SeedRuntime(value)) if value.operation == SeedRuntimeOperation::Status
    ));
    assert_native_seed_runtime_modes(&fixture);
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

fn assert_native_seed_runtime_modes(fixture: &Fixture) {
    for (mode, operation, binding) in [
        (
            "register",
            SeedRuntimeOperation::Register,
            Some("--request"),
        ),
        ("quotas-plan", SeedRuntimeOperation::QuotasPlan, None),
        ("quotas-apply", SeedRuntimeOperation::QuotasApply, None),
        (
            "quotas-reconcile",
            SeedRuntimeOperation::QuotasReconcile,
            None,
        ),
        (
            "departments-plan",
            SeedRuntimeOperation::DepartmentsPlan,
            None,
        ),
        (
            "departments-apply",
            SeedRuntimeOperation::DepartmentsApply,
            None,
        ),
        (
            "departments-reconcile",
            SeedRuntimeOperation::DepartmentsReconcile,
            None,
        ),
        (
            "departments-verify",
            SeedRuntimeOperation::DepartmentsVerify,
            None,
        ),
        (
            "identities-apply",
            SeedRuntimeOperation::IdentitiesApply,
            None,
        ),
        (
            "identities-verify",
            SeedRuntimeOperation::IdentitiesVerify,
            None,
        ),
        ("prepare", SeedRuntimeOperation::Prepare, None),
        ("start", SeedRuntimeOperation::Start, None),
        (
            "arm-input",
            SeedRuntimeOperation::ArmInput,
            Some("--request"),
        ),
        ("stop", SeedRuntimeOperation::Stop, None),
        ("status", SeedRuntimeOperation::Status, None),
        ("recover", SeedRuntimeOperation::Recover, None),
        (
            "recover-session",
            SeedRuntimeOperation::RecoverSession,
            Some("--producer-binding"),
        ),
    ] {
        let mut args = vec![
            "seed-runtime",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--operation",
            mode,
        ];
        if let Some(flag) = binding {
            args.extend([flag, Fixture::value(&fixture.first)]);
        }
        if mode != "status" {
            args.push("--write");
        }
        let parsed = fixture.parse(&args).unwrap();
        assert!(matches!(
            &parsed,
            CloneCommand::SeedRuntime(value)
                if value.operation == operation && !value.hold_originals
        ));
    }
}

#[test]
fn seed_runtime_build_requests_are_limited_to_department_plan_and_identity_verify() {
    let fixture = Fixture::new();
    for (operation, accepted) in [
        ("departments-plan", true),
        ("identities-verify", true),
        ("departments-apply", false),
        ("departments-reconcile", false),
        ("departments-verify", false),
        ("identities-apply", false),
        ("quotas-plan", false),
    ] {
        let parsed = fixture.parse(&[
            "seed-runtime",
            "--run-dir",
            Fixture::value(&fixture.run),
            "--operation",
            operation,
            "--request",
            Fixture::value(&fixture.first),
            "--write",
        ]);
        assert_eq!(parsed.is_ok(), accepted, "{operation}");
    }
}

#[test]
fn seed_runtime_held_originals_require_supported_modes_on_windows() {
    let fixture = Fixture::new();
    let run = Fixture::value(&fixture.run);
    for (mode, originals, supported) in [
        ("quotas-plan", "held", true),
        ("quotas-apply", "held", true),
        ("quotas-plan", "full", false),
        ("quotas-apply", "full", false),
        ("quotas-reconcile", "held", false),
        ("departments-plan", "held", true),
        ("departments-apply", "held", true),
        ("departments-verify", "held", true),
        ("departments-plan", "full", false),
        ("departments-reconcile", "held", false),
        ("identities-apply", "held", false),
        ("identities-reconcile", "held", false),
        ("identities-verify", "held", true),
        ("identities-verify", "full", false),
        ("prepare", "held", false),
        ("start", "held", true),
        ("start", "full", false),
        ("close", "held", true),
        ("close", "full", false),
        ("stop", "held", false),
        ("recover", "held", false),
    ] {
        let request = matches!(mode, "identities-reconcile" | "identities-verify")
            .then_some(fixture.first.as_path());
        let mut args = vec![
            "seed-runtime",
            "--run-dir",
            run,
            "--operation",
            mode,
            "--originals",
            originals,
            "--write",
        ];
        if let Some(path) = request {
            args.extend(["--request", Fixture::value(path)]);
        }
        let parsed = fixture.parse(&args);
        if cfg!(windows) && supported {
            assert!(
                matches!(
                    parsed,
                    Ok(CloneCommand::SeedRuntime(options))
                        if options.operation.as_str() == mode && options.hold_originals
                            && options.request.as_deref() == request
                ),
                "{mode} {originals}"
            );
        } else {
            let error = parsed.expect_err("当前 seed 阶段或参数不允许持有原件");
            assert!(error.contains("seed-runtime --originals"), "{error}");
        }
    }
}

#[test]
fn seed_close_requires_explicit_write_and_keeps_the_selected_run() {
    let fixture = Fixture::new();
    let mut args = vec![
        "seed-runtime",
        "--run-dir",
        Fixture::value(&fixture.run),
        "--operation",
        "close",
    ];
    let error = fixture.parse(&args).expect_err("seed close 必须显式授权");
    assert!(error.contains("必须显式传入 --write"), "{error}");

    args.push("--write");
    let CloneCommand::SeedRuntime(options) = fixture.parse(&args).unwrap() else {
        panic!("未解析为 seed-runtime 请求");
    };
    assert_eq!(options.operation, SeedRuntimeOperation::Close);
    assert_eq!(options.run_dir, fixture.run);
    assert!(!options.hold_originals);
    assert!(options.request.is_none());
    assert!(options.producer_binding.is_none());
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
        let effect = match fixture.parse(&arguments).unwrap() {
            CloneCommand::Stage { stage, .. } => stage.effect(),
            CloneCommand::Runtime(options) => options.operation.effect(),
            CloneCommand::PostCopy(options) => options.operation.effect(),
            CloneCommand::SeedRuntime(options) => options.operation.effect(),
            CloneCommand::Cache(options) => options.operation.effect(),
            command => panic!("意外的复制命令：{command:?}"),
        };
        assert_eq!(effect, expected);
    }
}

#[test]
fn parses_clone_init_bridge_and_maintenance_with_exact_paths() {
    let fixture = Fixture::new();
    let run_dir = fixture.directory.join("new-run");
    assert_eq!(
        fixture.parse(&[
            "init",
            "--manifest",
            Fixture::value(&fixture.first),
            "--run-dir",
            Fixture::value(&run_dir),
            "--write",
        ]),
        Ok(CloneCommand::Init {
            manifest: fixture.first.clone(),
            run_dir,
        })
    );

    let bridge = fixture.directory.join("bridge.json");
    assert_eq!(
        fixture.parse(&[
            "bridge",
            "--build",
            Fixture::value(&fixture.first),
            "--inventory",
            Fixture::value(&fixture.second),
            "--output",
            Fixture::value(&bridge),
            "--write",
        ]),
        Ok(CloneCommand::Bridge {
            build: fixture.first.clone(),
            inventory: fixture.second.clone(),
            output: bridge,
        })
    );

    let maintenance = fixture.directory.join("maintenance");
    assert_eq!(
        fixture.parse(&[
            "maintenance",
            "--operation",
            "build",
            "--output",
            Fixture::value(&maintenance),
            "--write",
        ]),
        Ok(CloneCommand::MaintenanceBuild {
            output: maintenance
        })
    );
    assert_eq!(
        fixture.parse(&[
            "maintenance",
            "--operation",
            "verify",
            "--output",
            Fixture::value(&fixture.first),
        ]),
        Ok(CloneCommand::MaintenanceVerify {
            output: fixture.first.clone()
        })
    );
}
