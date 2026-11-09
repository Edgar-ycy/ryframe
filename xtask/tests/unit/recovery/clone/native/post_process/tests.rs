use std::fs;

use serde_json::json;

use super::{command, inspection, lock, protocol};

#[test]
fn seed_recovery_derives_identity_lock_only_from_the_registered_identity_producer() {
    use super::super::{digest, json as document, resources::common};
    let (_temporary, backend, run) = formal_fixture();
    let plan_file = run.join("identity-plan.json");
    let mut plan = json!({"format_version":1,"kind":"devex-identities"});
    let semantic = digest::plan_hash(&plan).unwrap();
    plan["plan_sha256"] = json!(semantic);
    document::write_new(&plan_file, &plan).unwrap();
    let binding = common::binding(&plan_file).unwrap();
    assert_ne!(binding["sha256"], semantic);
    let identities = run.join("seed-runtime/identities");
    let request = json!({"identity_state":identities,"identity_plan":binding});
    let process = json!({"identity_lock_owner":"owner","identity":{"pid":123}});
    for mode in ["identities-apply", "identities-verify"] {
        let attempt = json!({"stage":"seed-runtime","mode":mode});
        let lock =
            protocol::identity_lock_request(&backend, &run, &attempt, &request, &process).unwrap();
        assert_eq!(
            lock,
            json!({"backend":backend,"identity_state":identities,"identity":{"pid":123},
                "owner_token":"owner","plan_sha256":semantic})
        );
        protocol::release_identity_lock(&backend, &run, &lock).unwrap();
        assert!(!identities.exists());
    }
    for (stage, mode) in [("seed-runtime", "quotas-apply"), ("post-copy", "schedules")] {
        let attempt = json!({"stage":stage,"mode":mode});
        assert!(
            protocol::identity_lock_request(&backend, &run, &attempt, &request, &process)
                .unwrap()
                .is_null()
        );
    }
}

#[test]
fn failed_unpublished_seed_registration_allows_explicit_resume_only_without_producer_evidence() {
    use super::super::lifecycle::state;
    let (_temporary, backend, run) = formal_fixture();
    let (number, _) = state::begin(&run, "seed-runtime", "register", &json!({})).unwrap();
    state::finish(&run, number, None, Some("StageError")).unwrap();
    let original = fs::read(run.join("state.json")).unwrap();
    super::require_quiet(&backend, &run, None).unwrap();
    let evidence = run
        .join("seed-runtime")
        .join(format!("attempt-{number:04}"));
    fs::create_dir_all(&evidence).unwrap();
    fs::write(
        evidence.join("session.stderr.log"),
        b"unknown process evidence",
    )
    .unwrap();
    assert!(super::require_quiet(&backend, &run, None).is_err());
    assert_eq!(fs::read(run.join("state.json")).unwrap(), original);
}

#[test]
fn formal_fixed_request_modes_refuse_python_dispatch_and_bad_options_without_stage_writes() {
    use crate::{
        cli::{CloneCommand, PostCopyOperation, PostCopyOptions},
        recovery::clone,
    };
    let (_temporary, backend, run) = formal_fixture();
    let input = run.parent().unwrap().join("input.json");
    let original = fs::read(run.join("state.json")).unwrap();
    for operation in [
        PostCopyOperation::Register,
        PostCopyOperation::Prepare,
        PostCopyOperation::Amend,
        PostCopyOperation::Schedules,
        PostCopyOperation::Reconcile,
        PostCopyOperation::Verify,
        PostCopyOperation::RecoverSession,
    ] {
        let options = PostCopyOptions {
            run_dir: run.clone(),
            operation,
            request: if matches!(
                operation,
                PostCopyOperation::Prepare
                    | PostCopyOperation::Schedules
                    | PostCopyOperation::Reconcile
                    | PostCopyOperation::Verify
            ) {
                Some(input.clone())
            } else {
                None
            },
            producer_binding: None,
        };
        assert!(
            clone::protocol(&CloneCommand::PostCopy(options.clone()), &backend)
                .unwrap_err()
                .to_string()
                .contains("原生持锁正式入口")
        );
        let error = super::entry::run(&backend, &options)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(match operation {
                PostCopyOperation::Register | PostCopyOperation::Amend => "需要显式请求文件",
                PostCopyOperation::RecoverSession => "回收需要明确生产者收据描述文件",
                _ => "此复制后阶段沿用已登记请求",
            }),
            "{error}"
        );
        assert_eq!(fs::read(run.join("state.json")).unwrap(), original);
        for name in [
            "run.lock",
            "post-copy.json",
            "post-copy",
            "seed-runtime.json",
        ] {
            assert!(!run.join(name).exists());
        }
    }
    let number = super::super::lifecycle::state::begin(&run, "post-copy", "register", &json!({}))
        .unwrap()
        .0;
    super::super::lifecycle::state::finish(
        &run,
        number,
        Some(&json!({"status":"post_copy_registered"})),
        None,
    )
    .unwrap();
    let before = fs::read(run.join("state.json")).unwrap();
    let options = PostCopyOptions {
        run_dir: run.clone(),
        operation: PostCopyOperation::Register,
        request: Some(input),
        producer_binding: None,
    };
    assert!(
        super::entry::run(&backend, &options)
            .unwrap_err()
            .to_string()
            .contains("唯一成功结果")
    );
    assert_eq!(fs::read(run.join("state.json")).unwrap(), before);
}

#[test]
fn amendment_keeps_complete_request_and_frozen_os_environment_without_exposing_values() {
    use super::super::resources::common;
    let backend = crate::workspace::root_dir();
    fs::create_dir_all(backend.join(".local-tests")).unwrap();
    let temporary = tempfile::Builder::new()
        .prefix("post-amend-environment-")
        .tempdir_in(backend.join(".local-tests"))
        .unwrap();
    let original = temporary.path().join("original.json");
    let corrected = temporary.path().join("corrected.json");
    let environment =
        json!({"APP_ENV":"test","TEMP":"D:/PrivateSentinel","TMP":"D:/PrivateSentinel"});
    super::super::json::write_new(&original, &json!({"environment":environment})).unwrap();
    super::super::json::write_new(&corrected, &json!({"environment":environment})).unwrap();
    let previous = json!({"api_environment":common::binding(&original).unwrap(),"source_admin":{"subject_id":"9"},"other":{"fixed":true}});
    let mut next = previous.clone();
    next["api_environment"] = common::binding(&corrected).unwrap();
    super::amend::candidate(&backend, &previous, &next).unwrap();
    let bytes = fs::read(&original).unwrap();
    let mut changed = next.clone();
    changed["source_admin"]["subject_id"] = json!("10");
    assert!(
        super::amend::candidate(&backend, &previous, &changed)
            .unwrap_err()
            .to_string()
            .contains("其他请求字段")
    );
    assert!(
        super::amend::candidate(&backend, &previous, &previous)
            .unwrap_err()
            .to_string()
            .contains("明确重新绑定")
    );
    for mapping in [
        json!({"APP_ENV":"test"}),
        json!({"APP_ENV":"test","TEMP":"other-private","TMP":"D:/PrivateSentinel"}),
        json!({"APP_ENV":"test","APP_CORS_ALLOW_ORIGINS":"PrivateSentinel"}),
        json!({"APP_ENV":"test","TEMP":false}),
    ] {
        fs::write(
            &corrected,
            serde_json::to_vec(&json!({"environment":mapping})).unwrap(),
        )
        .unwrap();
        next["api_environment"] = common::binding(&corrected).unwrap();
        let error = super::amend::candidate(&backend, &previous, &next)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("PrivateSentinel") && !error.contains("other-private"));
        assert_eq!(fs::read(&original).unwrap(), bytes);
    }
}

#[test]
fn fixed_request_stage_diagnostics_preserve_evidence_and_hide_unknown_private_causes() {
    use crate::recovery::clone::native::lifecycle::stage::{self, StageRequest};
    struct Rejected(&'static str);
    impl stage::StageRunner for Rejected {
        fn preflight(&self, _: &StageRequest<'_>) -> crate::Result<()> {
            Ok(())
        }
        fn execute(&self, request: &StageRequest<'_>) -> crate::Result<serde_json::Value> {
            super::diagnostic::context(
                request,
                super::diagnostic::Phase::Preparation,
                Err(self.0.into()),
            )
        }
    }
    for cause in [
        "准备API前目标完整复制后像变化",
        "password=PrivateSentinel token=HiddenToken",
    ] {
        let (_temporary, backend, run) = formal_fixture();
        let message = stage::run_named(&backend, &run, "post-copy", "prepare", &Rejected(cause))
            .unwrap_err()
            .to_string();
        assert!(message.contains("PostCopyPreparationRejected") && message.contains("preparation"));
        assert!(message.contains("failure-0001.json") && message.contains("不自动重放"));
        assert!(!message.contains("PrivateSentinel") && !message.contains("HiddenToken"));
        let state = super::super::lifecycle::state::load(&run, true).unwrap();
        assert_eq!(state["attempts"][0]["status"], "failed");
        assert_eq!(
            state["attempts"][0]["error_type"],
            "PostCopyPreparationRejected"
        );
        let evidence = super::super::json::read(&run.join("failure-0001.json")).unwrap();
        assert_eq!(evidence["kind"], "devex-stage-failure");
        assert_eq!(evidence["mode"], "prepare");
        assert_eq!(
            evidence["controller"],
            super::super::resources::common::binding(
                &super::super::lifecycle::state::controller_path(&run, 1)
            )
            .unwrap()
        );
        if cause.starts_with("准备API") {
            assert!(message.contains("完整数据库后像"));
        }
    }
}

#[test]
fn producer_plan_keeps_credentials_out_of_argv() {
    let root = tempfile::tempdir().unwrap();
    let backend = root.path();
    let source = backend.join("xtask/src/data/performance_identities/producer.rs");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "// source\n").unwrap();
    let tool = backend.join("xtask.exe");
    fs::write(&tool, "tool").unwrap();
    let request = json!({"backend":backend,"run_dir":backend.join("run"),"attempt":2,"kind":"identity-apply","output":backend.join("run/seed-runtime/attempt-0002"),"tool":{"path":tool,"sha256":"a".repeat(64)},"registration":{"identity_plan":{"path":backend.join("plan.json"),"sha256":"b".repeat(64)},"identity_state":backend.join("identities"),"password":"Private-Secret"},"configuration":null});
    let value = command::build(&request).unwrap();
    let encoded = serde_json::to_string(&value["argv"]).unwrap();
    assert!(!encoded.contains("Private-Secret"));
    assert!(encoded.contains("--identity-plan-sha256"));
}

#[test]
fn identity_release_is_permanent_and_idempotent() {
    let base = std::env::current_dir().unwrap().join(".local-tests/tmp");
    fs::create_dir_all(&base).unwrap();
    let root = tempfile::Builder::new()
        .prefix("post-lock-")
        .tempdir_in(base)
        .unwrap();
    let backend = root.path();
    let state = backend.join("identities");
    fs::create_dir(&state).unwrap();
    let identity = json!({"pid":4000,"started":"1000","executable":backend.join("xtask.exe")});
    let owner = uuid::Uuid::new_v4().to_string();
    let plan = "c".repeat(64);
    let value = json!({"format_version":1,"kind":"devex-identity-run-lock","pid":4000,"plan_sha256":plan,"owner_token":owner});
    fs::write(
        state.join("lock"),
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
    let request = json!({"backend":backend,"identity_state":state,"identity":identity,"owner_token":owner,"plan_sha256":plan});
    assert_eq!(lock::recover(&request).unwrap()["released"], true);
    let resumed = lock::recover(&request).unwrap();
    assert_eq!(resumed["released"], false);
    assert_eq!(resumed["idempotent"], true);
    assert!(!state.join("lock").exists());
}

#[test]
fn unsupported_kind_fails_before_a_command_is_built() {
    let root = tempfile::tempdir().unwrap();
    let request = json!({"backend":root.path(),"run_dir":root.path().join("run"),"attempt":1,"kind":"unknown","output":root.path().join("out"),"tool":{"path":root.path().join("tool"),"sha256":"d".repeat(64)},"registration":{},"configuration":null});
    assert!(command::build(&request).is_err());
}

#[test]
fn pid_reuse_only_accepts_a_strictly_newer_generation() {
    let old = json!({"pid":44,"started":"100","executable":"D:/tool.exe"});
    assert!(inspection::newer(
        &old,
        &json!({"pid":44,"started":"101","executable":"D:/other.exe"})
    ));
    assert!(!inspection::newer(
        &old,
        &json!({"pid":45,"started":"101","executable":"D:/tool.exe"})
    ));
    assert!(!inspection::newer(
        &old,
        &json!({"pid":44,"started":"99","executable":"D:/tool.exe"})
    ));
    assert!(!inspection::newer(
        &old,
        &json!({"pid":44,"started":"unknown","executable":"D:/tool.exe"})
    ));
}

#[test]
fn producer_argv_and_cleanup_refuse_a_reused_current_pid() {
    let current = std::process::id();
    let fake = json!({"pid":current,"started":"1","executable":std::env::current_exe().unwrap()});
    assert!(protocol::terminate_process(&json!({"identity":fake})).is_err());
    #[cfg(windows)]
    {
        let expected = json!(std::env::args().collect::<Vec<_>>());
        let error = inspection::require_actual_argv(&fake, &expected).unwrap_err();
        assert_eq!(error.to_string(), "登记进程已退出或内核创建身份变化");
    }
}

#[test]
fn owners_must_be_uuid_v4() {
    assert!(inspection::uuid(&json!(uuid::Uuid::new_v4().to_string())));
    assert!(!inspection::uuid(&json!(
        "10000000-0000-1000-8000-000000000001"
    )));
    assert!(!inspection::uuid(&json!(false)));
}

#[test]
fn producer_kind_is_bound_to_one_stage_and_mode() {
    assert!(command::accepts("identity-apply", "identities-apply"));
    assert!(!command::accepts("identity-apply", "identities-verify"));
    assert!(command::accepts("session", "reconcile"));
    assert!(!command::accepts("session", "verify"));
}

#[test]
fn identity_release_rejects_another_owner_without_modifying_lock() {
    let base = std::env::current_dir().unwrap().join(".local-tests/tmp");
    fs::create_dir_all(&base).unwrap();
    let root = tempfile::Builder::new()
        .prefix("post-lock-owner-")
        .tempdir_in(base)
        .unwrap();
    let backend = root.path();
    let state = backend.join("identities");
    fs::create_dir(&state).unwrap();
    let owner = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let plan = "e".repeat(64);
    let value = json!({"format_version":1,"kind":"devex-identity-run-lock","pid":4001,"plan_sha256":plan,"owner_token":owner});
    let bytes = format!("{}\n", serde_json::to_string_pretty(&value).unwrap());
    fs::write(state.join("lock"), &bytes).unwrap();
    let request = json!({"backend":backend,"identity_state":state,"identity":{"pid":4001,"started":"1001","executable":backend.join("xtask.exe")},"owner_token":other,"plan_sha256":plan});
    assert!(lock::recover(&request).is_err());
    assert_eq!(fs::read(state.join("lock")).unwrap(), bytes.as_bytes());
}

#[test]
fn identity_release_resumes_after_lock_claim_interruption() {
    let base = std::env::current_dir().unwrap().join(".local-tests/tmp");
    fs::create_dir_all(&base).unwrap();
    let root = tempfile::Builder::new()
        .prefix("post-lock-claim-interrupt-")
        .tempdir_in(base)
        .unwrap();
    let backend = root.path();
    let state = backend.join("identities");
    let releases = state.join("lock-releases");
    fs::create_dir_all(&releases).unwrap();
    let owner = uuid::Uuid::new_v4().to_string();
    let plan = "f".repeat(64);
    let identity = json!({"pid":4002,"started":"1002","executable":backend.join("xtask.exe")});
    let value = json!({"format_version":1,"kind":"devex-identity-run-lock","pid":4002,"plan_sha256":plan,"owner_token":owner});
    let release = uuid::Uuid::new_v4();
    fs::write(
        releases.join(format!("{release}.lock")),
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
    let request = json!({"backend":backend,"identity_state":state,"identity":identity,"owner_token":owner,"plan_sha256":plan});
    let resumed = lock::recover(&request).unwrap();
    assert_eq!(resumed["released"], true);
    assert_eq!(resumed["idempotent"], false);
    assert!(releases.join(format!("{release}.released.json")).is_file());
}

#[test]
fn identity_release_resumes_after_pending_marker_interruption() {
    let base = std::env::current_dir().unwrap().join(".local-tests/tmp");
    fs::create_dir_all(&base).unwrap();
    let root = tempfile::Builder::new()
        .prefix("post-lock-marker-interrupt-")
        .tempdir_in(base)
        .unwrap();
    let backend = root.path();
    let state = backend.join("identities");
    fs::create_dir(&state).unwrap();
    let owner = uuid::Uuid::new_v4().to_string();
    let plan = "1".repeat(64);
    let identity = json!({"pid":4003,"started":"1003","executable":backend.join("xtask.exe")});
    let value = json!({"format_version":1,"kind":"devex-identity-run-lock","pid":4003,"plan_sha256":plan,"owner_token":owner});
    fs::write(
        state.join("lock"),
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
    let request = json!({"backend":backend,"identity_state":state,"identity":identity,"owner_token":owner,"plan_sha256":plan});
    assert_eq!(lock::recover(&request).unwrap()["released"], true);
    let releases = state.join("lock-releases");
    let marker = fs::read_dir(&releases)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".released.json"))
        .unwrap();
    let pending = marker.with_file_name(
        marker
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace(".released.json", ".released.json.pending"),
    );
    fs::rename(&marker, &pending).unwrap();
    let resumed = lock::recover(&request).unwrap();
    assert_eq!(resumed["released"], true);
    assert_eq!(resumed["idempotent"], false);
    assert!(marker.is_file());
    assert!(!pending.exists());
}

fn formal_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    use super::super::{json as document, lifecycle::state, resources::common};
    let backend = crate::workspace::root_dir();
    fs::create_dir_all(backend.join(".local-tests")).unwrap();
    let temporary = tempfile::Builder::new()
        .prefix("post-copy-entry-")
        .tempdir_in(backend.join(".local-tests"))
        .unwrap();
    let run = temporary.path().join("run");
    fs::create_dir(&run).unwrap();
    let input = temporary.path().join("input.json");
    document::write_new(&input, &json!({"fixture":true})).unwrap();
    let binding = common::binding(&input).unwrap();
    document::write_new(
        &run.join("manifest.json"),
        &json!({"format_version":1,
        "kind":"devex-clone-run","id":"post-native-unit","source_request":binding,
        "source_export":null,"initialized":binding,"source_environment":binding,
        "target_environment":binding,"copy_directory":temporary.path().join("copy"),
        "copy_stage":"source_to_seed","build_bridges":[]}),
    )
    .unwrap();
    state::initialize(&run).unwrap();
    (temporary, backend, run)
}
