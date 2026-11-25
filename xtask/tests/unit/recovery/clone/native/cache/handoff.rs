//! 正式交接入口、真实控制互斥与失败前零启动；完整服务交接另由隔离场景验收。
use super::{locks, run, selected};
use crate::{
    cli::{CloneRole, CloneRuntimeOperation, CloneRuntimeOptions, CloneSide},
    recovery::clone::native::{json as document, lifecycle::state, resources::common},
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

#[test]
fn published_collectors_protect_the_sibling_manifest_and_original_fixture() {
    use crate::recovery::{
        clone::native::{post_process, target_verify::guard::Inputs},
        fixture_control::source_pair::execution::tests::Fixture as PublishedFixture,
    };
    use std::collections::BTreeSet;
    let fixture = PublishedFixture::new();
    let publication = fixture.publication();
    let backend = fixture.output.join("backend");
    let local = backend.join(".local-tests");
    fs::create_dir(&local).unwrap();
    let owned = local.join("owned.json");
    document::write_new(&owned, &json!({})).unwrap();
    let manifest = fixture.output.join("execution-source.json");
    let original = fixture.receipt.clone();
    let external = fixture.backend.join(".local-tests/private.toml");
    let request = json!({"execution_backend":{"path":backend,"execution_source":fixture.binding()},
        "owned":common::binding(&owned).unwrap(),
        "tool":{"path":external,"sha256":common::binding(&external).unwrap()["sha256"]}});
    let bytes = fs::read(&original).unwrap();
    assert_eq!(
        publication["original"]["fixture_receipt"],
        common::binding(&original).unwrap()
    );
    for collect in [super::input::bindings, post_process::files::bindings] {
        let mut paths = BTreeSet::new();
        collect(&backend, &request, &mut paths).unwrap();
        assert_eq!(paths.len(), 4);
        for expected in [&manifest, &original, &owned, &external] {
            assert!(paths.iter().any(|path| {
                fs::canonicalize(path).unwrap() == fs::canonicalize(expected).unwrap()
            }));
        }
        let protected = Inputs::open(paths).unwrap();
        protected.check().unwrap();
        if fs::write(&original, b"original fixture drift").is_ok() {
            assert!(protected.check().is_err());
        } else {
            assert_eq!(fs::read(&original).unwrap(), bytes);
        }
        drop(protected);
        fs::write(&original, &bytes).unwrap();
    }
    assert_eq!(fs::read(original).unwrap(), bytes);
}

#[test]
fn published_collectors_reject_relocated_or_mixed_proofs_without_writing_inputs() {
    use crate::recovery::{
        clone::native::post_process,
        fixture_control::source_pair::execution::tests::Fixture as PublishedFixture,
    };
    use std::collections::BTreeSet;
    let fixture = PublishedFixture::new();
    fixture.publication();
    let backend = fixture.output.join("backend");
    let local = backend.join(".local-tests");
    fs::create_dir(&local).unwrap();
    let manifest = fixture.output.join("execution-source.json");
    let bytes = fs::read(&manifest).unwrap();
    let original = fs::read(&fixture.receipt).unwrap();
    let relocated = local.join("same-content.json");
    fs::write(&relocated, &bytes).unwrap();
    let declared = json!({"path":backend,"execution_source":fixture.binding()});
    let mut moved = declared.clone();
    moved["execution_source"] = common::binding(&relocated).unwrap();
    let mut mixed = declared.clone();
    mixed["fixture"] = common::binding(&fixture.receipt).unwrap();
    let mut drifted = declared.clone();
    drifted["execution_source"]["sha256"] = json!("0".repeat(64));
    let before = fs::read_dir(&local)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<BTreeSet<_>>();
    for collect in [super::input::bindings, post_process::files::bindings] {
        collect(
            &backend,
            &json!({"execution_backend":declared}),
            &mut BTreeSet::new(),
        )
        .unwrap();
        for invalid in [
            json!({"execution_backend":moved}),
            json!({"execution_backend":mixed}),
            json!({"execution_backend":drifted}),
            json!({"ordinary":fixture.binding()}),
            json!({"other":{"execution_source":fixture.binding()}}),
        ] {
            assert!(collect(&backend, &invalid, &mut BTreeSet::new()).is_err());
        }
        assert_eq!(fs::read(&manifest).unwrap(), bytes);
        assert_eq!(fs::read(&fixture.receipt).unwrap(), original);
        fs::write(&manifest, b"{}\n").unwrap();
        let request = json!({"execution_backend":declared});
        assert!(collect(&backend, &request, &mut BTreeSet::new()).is_err());
        assert_eq!(fs::read(&manifest).unwrap(), b"{}\n");
        fs::write(&manifest, &bytes).unwrap();
        collect(&backend, &request, &mut BTreeSet::new()).unwrap();
        assert_eq!(fs::read(&relocated).unwrap(), bytes);
        assert_eq!(
            fs::read_dir(&local)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<BTreeSet<_>>(),
            before
        );
    }
}

struct Fixture {
    temporary: tempfile::TempDir,
    backend: PathBuf,
    run: PathBuf,
    target: PathBuf,
    runtime: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let temporary = tempfile::Builder::new()
            .prefix("cache-api-handoff-")
            .tempdir_in(backend.join(".local-tests"))
            .unwrap();
        let run = temporary.path().join("run");
        let target = temporary.path().join("target");
        let runtime = run.join("post-copy/runtime");
        for path in [&run, &target, &runtime] {
            fs::create_dir_all(path).unwrap();
        }
        let source = temporary.path().join("source.json");
        let initial = target.join("initialized.json");
        let environment = temporary.path().join("environment.json");
        document::write_new(&source, &json!({"fixture":true})).unwrap();
        document::write_new(&initial, &json!({"fixture":true})).unwrap();
        document::write_new(
            &environment,
            &json!({"environment":{"APP_ENV":"test",
            "TEMP":temporary.path(),"TMP":temporary.path()}}),
        )
        .unwrap();
        let copy = temporary.path().join("copy");
        fs::create_dir_all(copy.join("ledger")).unwrap();
        document::write_new(
            &run.join("manifest.json"),
            &json!({
            "format_version":1,"kind":"devex-clone-run","id":"handoff-unit",
            "source_request":common::binding(&source).unwrap(),"source_export":null,
            "initialized":common::binding(&initial).unwrap(),
            "source_environment":common::binding(&environment).unwrap(),
            "target_environment":common::binding(&environment).unwrap(),
            "copy_directory":copy,"copy_stage":"source_to_seed","build_bridges":[]}),
        )
        .unwrap();
        state::initialize(&run).unwrap();
        Self {
            temporary,
            backend,
            run,
            target,
            runtime,
        }
    }

    fn options(&self) -> CloneRuntimeOptions {
        CloneRuntimeOptions {
            run_dir: self.run.clone(),
            side: CloneSide::Target,
            operation: CloneRuntimeOperation::Start,
            roles: vec![CloneRole::Api],
        }
    }

    fn registration_file(&self) {
        document::write_new(&self.run.join("post-copy.json"), &json!({"fixture":true})).unwrap();
    }

    fn published_runtime(&self) {
        self.published_runtime_at(&self.backend, None);
    }

    fn published_runtime_at(&self, product: &std::path::Path, build: Option<Value>) {
        use crate::recovery::clone::native::lifecycle::stage;
        let listeners = (0..2)
            .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
            .collect::<Vec<_>>();
        let ports = listeners
            .iter()
            .map(|listener| listener.local_addr().unwrap().port())
            .collect::<Vec<_>>();
        for (name, id) in [("config", 1), ("worker-config", 2)] {
            let directory = self.runtime.join(name);
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join("app.test.toml"), format!(
                "scope_id=\"handoff-control-tests\"\nsnowflake_worker_id={id}\n[app]\nhost=\"127.0.0.1\"\nport={}\n[jobs]\nmode=\"external\"\nhealth_host=\"127.0.0.1\"\nhealth_port={}\n", ports[0],ports[1])).unwrap();
        }
        let executable = std::env::current_exe().unwrap();
        let product_build = build
            .as_ref()
            .map(|binding| common::read_bound(&self.backend, binding, "测试产品构建").unwrap());
        let api = product_build.as_ref().map_or_else(
            || json!(executable),
            |value| value["artifacts"]["api"]["executable"].clone(),
        );
        let worker = product_build.as_ref().map_or_else(
            || json!(executable),
            |value| value["artifacts"]["worker"]["executable"].clone(),
        );
        document::write_new(
            &self.runtime.join("binaries.json"),
            &json!({"ryframe":api,
            "ryframe-worker":worker,"ryframe-reset":executable,"ryframe-migrate":executable}),
        )
        .unwrap();
        let runtime = crate::recovery::fixture_control::full_stack_runtime_contract(
            product,
            &self.runtime,
            &self.runtime.join("config"),
            &self.runtime.join("worker-config"),
        )
        .unwrap();
        document::write_new(&self.runtime.join("runtime.json"), &runtime).unwrap();
        let environment = self.temporary.path().join("environment.json");
        let post = self.run.join("post-copy.json");
        let mut request = json!({"format_version":1,"kind":"devex-clone-post-copy",
            "api_environment":common::binding(&environment).unwrap()});
        if let Some(build) = build {
            request["backend_build"] = build;
        }
        document::write_new(&post, &request).unwrap();
        stage::run_named(&self.backend, &self.run, "post-copy", "register", &Published(
            json!({"status":"post_copy_registered","registration":common::binding(&post).unwrap()}))).unwrap();
        let handoff = self.runtime.join("handoff.json");
        document::write_new(
            &handoff,
            &json!({"registration":common::binding(&post).unwrap(),
            "runtime":common::binding(&self.runtime.join("runtime.json")).unwrap(),
            "binaries":common::binding(&self.runtime.join("binaries.json")).unwrap(),
            "api_url":format!("http://127.0.0.1:{}",ports[0]),"worker_started":false}),
        )
        .unwrap();
        stage::run_named(&self.backend, &self.run, "post-copy", "prepare", &Published(
            json!({"status":"post_copy_api_prepared","handoff":common::binding(&handoff).unwrap(),"worker_started":false}))).unwrap();
        drop(listeners);
    }
}

struct Published(Value);
impl crate::recovery::clone::native::lifecycle::stage::StageRunner for Published {
    fn preflight(
        &self,
        _: &crate::recovery::clone::native::lifecycle::stage::StageRequest<'_>,
    ) -> crate::Result<()> {
        Ok(())
    }
    fn execute(
        &self,
        _: &crate::recovery::clone::native::lifecycle::stage::StageRequest<'_>,
    ) -> crate::Result<Value> {
        if self.0.is_null() {
            return Err("fixture-post-registration-final-check-failed".into());
        }
        Ok(self.0.clone())
    }
}

#[test]
fn registered_target_lifecycle_selects_native_handoff_and_rejects_extra_roles() {
    let fixture = Fixture::new();
    let mut options = fixture.options();
    assert!(!selected(&fixture.backend, &options).unwrap());
    fixture.registration_file();
    assert!(selected(&fixture.backend, &options).unwrap());
    let original = fs::read(fixture.run.join("state.json")).unwrap();
    options.roles.push(CloneRole::Worker);
    assert!(
        run(&fixture.backend, &options)
            .unwrap_err()
            .to_string()
            .contains("Worker必须保持停止")
    );
    assert_eq!(fs::read(fixture.run.join("state.json")).unwrap(), original);
    assert!(!fixture.run.join("run.lock").exists());
    options.roles = vec![CloneRole::Worker];
    assert!(run(&fixture.backend, &options).is_err());
    assert_eq!(fs::read(fixture.run.join("state.json")).unwrap(), original);
    options.side = CloneSide::Source;
    assert!(!selected(&fixture.backend, &options).unwrap());
    options.side = CloneSide::Target;
    for operation in [
        CloneRuntimeOperation::Stop,
        CloneRuntimeOperation::Status,
        CloneRuntimeOperation::Recover,
    ] {
        options.operation = operation;
        assert!(selected(&fixture.backend, &options).unwrap());
    }
}

#[test]
fn seed_retirement_requires_current_prepare_and_preserves_the_original_api_generation() {
    use super::observation::retirement::Retirement;
    use crate::recovery::clone::native::{lifecycle::stage, seed};

    let fixture = Fixture::new();
    fixture.published_runtime();
    let backend = &fixture.backend;
    let run = &fixture.run;
    let post = seed::history::post(backend, run, None, None, false).unwrap();
    let original = document::read(&fixture.runtime.join("handoff.json")).unwrap();
    let runtime = document::read(&fixture.runtime.join("runtime.json")).unwrap();
    let api = publish_original_api_generation(&fixture, &runtime);
    let scope = &runtime["scope_id"];
    let registration = run.join("seed-runtime.json");
    document::write_new(&registration, &json!({
        "format_version":1,"kind":"devex-clone-seed-runtime","run_manifest":state::manifest_binding(run).unwrap(),
        "post_copy":post.descriptor,"post_verify":null,"identity_plan":null,"identity_state":null,
        "identity_tool":null,"api_process":common::binding(&api).unwrap(),"node":null
    })).unwrap();
    let register = json!({"status":"seed_runtime_registered","registration":common::binding(&registration).unwrap()});
    stage::run_named(
        backend,
        run,
        "seed-runtime",
        "register",
        &Published(register),
    )
    .unwrap();
    let load = || Retirement::load(backend, run, &post, &state::load(run, true).unwrap());
    assert!(load().is_err());
    let seed_runtime = run.join("seed-runtime/runtime");
    fs::create_dir_all(&seed_runtime).unwrap();
    for name in ["runtime.json", "binaries.json"] {
        fs::copy(fixture.runtime.join(name), seed_runtime.join(name)).unwrap();
    }
    let handoff = seed_runtime.join("handoff.json");
    document::write_new(&handoff, &json!({
        "registration":common::binding(&registration).unwrap(),
        "api_environment":post.request["api_environment"],"backend_build":post.request["backend_build"],
        "original_api":common::binding(&api).unwrap(),"scope_id":scope,"api_url":original["api_url"],
        "runtime":common::binding(&seed_runtime.join("runtime.json")).unwrap(),
        "binaries":common::binding(&seed_runtime.join("binaries.json")).unwrap()
    })).unwrap();
    let prepared =
        json!({"status":"seed_runtime_prepared","handoff":common::binding(&handoff).unwrap()});
    let prepare = || {
        stage::run_named(
            backend,
            run,
            "seed-runtime",
            "prepare",
            &Published(prepared.clone()),
        )
        .unwrap()
    };
    prepare();
    assert!(load().unwrap().is_some());
    let api_before = fs::read(&api).unwrap();
    document::write_replace(&api, &json!({})).unwrap();
    assert!(load().is_err());
    fs::write(&api, api_before).unwrap();
    let unexpected = seed_runtime.join("worker.json");
    document::write_new(&unexpected, &json!({})).unwrap();
    assert!(load().is_err());
    fs::remove_file(&unexpected).unwrap();
    assert!(
        stage::run_named(
            backend,
            run,
            "seed-runtime",
            "prepare",
            &Published(Value::Null)
        )
        .is_err()
    );
    assert!(seed::runtime_model::inputs(backend, run).is_ok());
    assert!(load().is_err());
    prepare();
    assert!(
        stage::run_named(
            backend,
            run,
            "seed-runtime",
            "start",
            &Published(Value::Null)
        )
        .is_err()
    );
    assert!(load().is_err());
}

fn publish_original_api_generation(fixture: &Fixture, runtime: &Value) -> PathBuf {
    let directory =
        crate::recovery::fixture_control::portable_path(&fixture.runtime.canonicalize().unwrap());
    let api = fixture.runtime.join("api.json");
    let executable = &runtime["artifacts"]["api"]["path"];
    let identity = json!({"pid":40101,"started":"100","executable":executable});
    let scope = &runtime["scope_id"];
    document::write_new(
        &api,
        &json!({"format_version":1,"role":"api","scope_id":scope,"identity":identity}),
    )
    .unwrap();
    document::write_new(
        &fixture.runtime.join("api-tree.json"),
        &json!({
            "format_version":2,"kind":"full-stack-process-tree","runtime_directory":directory,
            "role":"api","scope_id":scope,"operation_id":"00000000000000000000000000000001",
            "process":identity,"group_id":40102,
            "supervisor":{"pid":40102,"started":"99","executable":executable},
            "monitor":{"pid":40103,"started":"98","executable":executable}
        }),
    )
    .unwrap();
    api
}

#[test]
fn missing_active_registration_fails_real_stage_releases_generation_and_never_starts_api() {
    let fixture = Fixture::new();
    fixture.registration_file();
    let initialized = fs::read(fixture.target.join("initialized.json")).unwrap();
    let error = run(&fixture.backend, &fixture.options())
        .unwrap_err()
        .to_string();
    assert!(error.contains("ApiHandoffBindingRejected"), "{error}");
    assert!(error.contains("post-copy.json"), "{error}");
    assert!(error.contains("failure-0001.json"), "{error}");
    assert!(
        error.contains("cargo xtask check recovery clone status --run-dir"),
        "{error}"
    );
    assert!(error.contains("本次尚未调用API启动"), "{error}");
    let saved = state::load(&fixture.run, true).unwrap();
    assert_eq!(saved["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(saved["attempts"][0]["stage"], "runtime-target");
    assert_eq!(saved["attempts"][0]["status"], "failed");
    assert_eq!(
        saved["attempts"][0]["error_type"],
        "ApiHandoffBindingRejected"
    );
    assert!(fixture.target.join("copy-target.guard").is_file());
    assert!(!fixture.target.join("initialize.lock").exists());
    assert!(!fixture.run.join("run.lock").exists());
    assert_eq!(
        fs::read(fixture.target.join("initialized.json")).unwrap(),
        initialized
    );
    for root in [&fixture.runtime, fixture.temporary.path()] {
        for name in [
            "api-control.lock",
            "api.json",
            "api-tree.json",
            "producer-history.json",
        ] {
            assert!(!root.join(name).exists());
        }
    }
}

#[test]
fn runtime_handoff_lock_blocks_competitors_and_preserves_changed_owner_instead_of_deleting_it() {
    let fixture = Fixture::new();
    let lock = locks::Runtime::acquire(&fixture.backend, &fixture.runtime).unwrap();
    let path = fixture.runtime.join("worker-control.lock/owner.json");
    let original = fs::read(&path).unwrap();
    assert!(locks::Runtime::acquire(&fixture.backend, &fixture.runtime).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    lock.finish(&fixture.backend, Ok(())).unwrap();
    assert!(!path.parent().unwrap().exists());
    let lock = locks::Runtime::acquire(&fixture.backend, &fixture.runtime).unwrap();
    let mut changed: Value = document::read(&path).unwrap();
    changed["token"] = json!("foreign-owner");
    document::write_replace(&path, &changed).unwrap();
    let bytes = fs::read(&path).unwrap();
    assert!(lock.finish(&fixture.backend, Ok(())).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(document::read(&path).unwrap(), changed);
}

struct DiagnosticFailure(u8);

impl crate::recovery::clone::native::lifecycle::stage::StageRunner for DiagnosticFailure {
    fn preflight(
        &self,
        _: &crate::recovery::clone::native::lifecycle::stage::StageRequest<'_>,
    ) -> crate::Result<()> {
        Ok(())
    }

    fn execute(
        &self,
        request: &crate::recovery::clone::native::lifecycle::stage::StageRequest<'_>,
    ) -> crate::Result<Value> {
        use super::diagnostic::{self, Phase};
        let cause: Box<dyn std::error::Error> = if self.0 == 0 {
            std::io::Error::from_raw_os_error(5).into()
        } else if self.0 == 2 {
            crate::recovery::clone::native::seed::source_registration::quiet::CimUnavailable {
                code: Some(0x80041003_u32 as i32),
            }
            .into()
        } else {
            "fixture-private-primary-token".into()
        };
        diagnostic::context(
            request.run_dir,
            request.attempt,
            if self.0 != 1 {
                Phase::BeforeStart
            } else {
                Phase::Start
            },
            Err(diagnostic::failure(
                if self.0 != 1 {
                    Phase::BeforeStart
                } else {
                    Phase::Start
                },
                cause,
                (self.0 == 1).then(|| "fixture-private-secondary-password".into()),
            )),
        )
    }
}

#[test]
fn formal_stage_reports_safe_phase_system_code_and_unknown_start_without_replay() {
    use crate::recovery::clone::native::lifecycle::stage;
    for failure_kind in [0, 1, 2] {
        let io_failure = failure_kind == 0;
        let unknown_start = failure_kind == 1;
        let fixture = Fixture::new();
        let error = stage::run_named(
            &fixture.backend,
            &fixture.run,
            "runtime-target",
            "start",
            &DiagnosticFailure(failure_kind),
        )
        .unwrap_err()
        .to_string();
        let category = if unknown_start {
            "ApiHandoffStartUnconfirmed"
        } else {
            "ApiHandoffPreStartRejected"
        };
        assert!(error.contains(category), "{error}");
        assert_eq!(error.contains("系统码：Some(5)"), io_failure, "{error}");
        assert_eq!(error.contains("API可能已经启动"), unknown_start, "{error}");
        if failure_kind == 2 {
            assert!(
                error.contains("0x80041003") && error.contains("ROOT\\CIMV2"),
                "{error}"
            );
        }
        assert_eq!(
            error.contains("锁或后置核验同时失败"),
            unknown_start,
            "{error}"
        );
        assert!(!error.contains("fixture-private"), "{error}");
        assert!(error.contains("failure-0001.json"), "{error}");
        assert!(error.contains("不自动重放"), "{error}");
        assert!(!error.contains("HistoricalProducerCimUnavailable"));
        let saved = state::load(&fixture.run, true).unwrap();
        assert_eq!(saved["attempts"][0]["status"], "failed");
        assert_eq!(saved["attempts"][0]["error_type"], category);
        assert!(!fixture.run.join("run.lock").exists());
        assert!(!fixture.runtime.join("api.json").exists());
    }
}

#[test]
fn formal_status_is_readonly_and_exact_stop_uses_frozen_environment_without_opposite_source() {
    let fixture = Fixture::new();
    fixture.published_runtime();
    let environment = ["APP_ENV", "TEMP", "TMP"].map(std::env::var_os);
    let before = fs::read_dir(&fixture.runtime)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<std::collections::BTreeSet<_>>();
    let mut options = fixture.options();
    options.operation = CloneRuntimeOperation::Status;
    let status = run(&fixture.backend, &options).unwrap();
    assert_eq!(status["result"]["kind"], "devex-clone-runtime-observation");
    assert_eq!(
        status["result"]["processes"],
        json!({"api":{"state":"stopped","identity":null,"ready":false}})
    );
    assert_eq!(
        fs::read_dir(&fixture.runtime)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<std::collections::BTreeSet<_>>(),
        before
    );
    crate::recovery::clone::native::lifecycle::stage::run_named(
        &fixture.backend,
        &fixture.run,
        "post-copy",
        "register",
        &Published(Value::Null),
    )
    .unwrap_err();
    // 清理只需要已发布的这一侧运行合同；失去原来源文件不授权重建或扩大停止范围。
    fs::remove_file(fixture.temporary.path().join("source.json")).unwrap();
    options.operation = CloneRuntimeOperation::Stop;
    let stopped = run(&fixture.backend, &options).unwrap();
    assert_eq!(stopped["result"]["operation"], "stop");
    assert_eq!(
        stopped["result"]["processes"],
        status["result"]["processes"]
    );
    assert_eq!(stopped["result"]["producer_history"], Value::Null);
    assert_eq!(
        ["APP_ENV", "TEMP", "TMP"].map(std::env::var_os),
        environment
    );
    for name in [
        "worker-control.lock",
        "api-control.lock",
        "producer-history.json",
        "api.json",
        "api-tree.json",
    ] {
        assert!(!fixture.runtime.join(name).exists(), "{name}");
    }
    assert!(fixture.runtime.join("api-control-receipts").is_dir());
    assert!(!fixture.runtime.join("worker-control-receipts").exists());
    let saved = state::load(&fixture.run, false).unwrap();
    assert_eq!(
        saved["attempts"].as_array().unwrap().last().unwrap()["status"],
        "passed"
    );
}

#[test]
fn pending_control_and_frozen_environment_drift_block_replay_and_history_publication() {
    let fixture = Fixture::new();
    fixture.published_runtime();
    let pending = fixture.runtime.join("api-control.lock");
    fs::create_dir(&pending).unwrap();
    let path = pending.join("request.json");
    fs::write(&path, b"fixture-private-unknown-password").unwrap();
    let mut options = fixture.options();
    for operation in [CloneRuntimeOperation::Status, CloneRuntimeOperation::Stop] {
        options.operation = operation;
        let error = run(&fixture.backend, &options).unwrap_err().to_string();
        assert!(error.contains("未完成控制证据"), "{error}");
        assert!(!error.contains("fixture-private"), "{error}");
        assert_eq!(
            fs::read(&path).unwrap(),
            b"fixture-private-unknown-password"
        );
        for name in ["result.json", "supervisor.json", "candidate.json"] {
            assert!(!pending.join(name).exists());
        }
        for name in [
            "producer-history.json",
            "api-control-receipts",
            "worker-control.lock",
        ] {
            assert!(!fixture.runtime.join(name).exists(), "{name}");
        }
    }
    fs::remove_file(&path).unwrap();
    fs::remove_dir(&pending).unwrap();
    let environment = fixture.temporary.path().join("environment.json");
    fs::write(&environment, b"fixture-private-environment-token").unwrap();
    let error = run(&fixture.backend, &options).unwrap_err().to_string();
    assert!(error.contains("ApiHandoffPreStopRejected"), "{error}");
    assert!(!error.contains("fixture-private"), "{error}");
    assert_eq!(
        fs::read(&environment).unwrap(),
        b"fixture-private-environment-token"
    );
    assert!(!pending.exists());
    assert!(!fixture.runtime.join("api-control-receipts").exists());
    assert!(!fixture.runtime.join("producer-history.json").exists());
}

#[test]
fn formal_recovery_preserves_live_or_unknown_owner_and_releases_only_original_reused_pid_lock() {
    let fixture = Fixture::new();
    fixture.published_runtime();
    let lock = locks::Runtime::acquire(&fixture.backend, &fixture.runtime).unwrap();
    // 模拟控制器退出但保留持久owner；测试进程本身仍活着，必须拒绝释放其创建代次。
    drop(lock);
    let formal_runtime = fixture.run.join("post-copy").join("runtime");
    assert_eq!(
        fs::canonicalize(&fixture.runtime).unwrap(),
        fs::canonicalize(&formal_runtime).unwrap()
    );
    let path = fixture.runtime.join("worker-control.lock/owner.json");
    let original = fs::read(&path).unwrap();
    let mut options = fixture.options();
    options.operation = CloneRuntimeOperation::Recover;
    let error = run(&fixture.backend, &options).unwrap_err().to_string();
    assert!(error.contains("控制器仍在运行"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), original);
    let mut owner = document::read(&path).unwrap();
    let mut other_directory = owner.clone();
    other_directory["runtime_directory"] = json!(fixture.target);
    document::write_replace(&path, &other_directory).unwrap();
    let rejected = fs::read(&path).unwrap();
    let error = run(&fixture.backend, &options).unwrap_err().to_string();
    assert!(error.contains("登记运行目录与当前运行目录不符"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), rejected);
    assert!(!fixture.runtime.join("api-control-receipts").exists());
    document::write_replace(&path, &owner).unwrap();
    owner["identity"]["started"] = json!("1");
    document::write_replace(&path, &owner).unwrap();
    let unknown = path.parent().unwrap().join("unknown.txt");
    fs::write(&unknown, b"fixture-private-lock-token").unwrap();
    let error = run(&fixture.backend, &options).unwrap_err().to_string();
    assert!(error.contains("单 owner 文件集合发生变化"), "{error}");
    assert!(!error.contains("fixture-private-lock-token"), "{error}");
    assert!(unknown.exists() && path.exists());
    fs::remove_file(&unknown).unwrap();
    let pending = fixture.runtime.join("api-control.lock");
    fs::create_dir(&pending).unwrap();
    assert!(run(&fixture.backend, &options).is_err());
    assert!(pending.exists() && path.exists());
    fs::remove_dir(&pending).unwrap();
    let recovered = run(&fixture.backend, &options).unwrap();
    assert_eq!(
        recovered["result"]["kind"],
        "devex-clone-runtime-lock-reconciliation"
    );
    assert_eq!(recovered["result"]["owner"], owner);
    assert_eq!(recovered["result"]["remote_writes_reconciled"], false);
    assert_eq!(
        recovered["result"]["observed_identity"],
        common::process_identity(std::process::id())
            .unwrap()
            .unwrap()
    );
    assert!(!path.parent().unwrap().exists());
    let receipts = fs::read_dir(&fixture.runtime)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("controller-reconcile-")
                .then_some(path)
        })
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 1);
    assert_eq!(document::read(&receipts[0]).unwrap(), recovered["result"]);
    assert!(!fixture.runtime.join("api-control-receipts").exists());
    assert!(!fixture.runtime.join("producer-history.json").exists());
}

#[test]
fn frozen_published_product_root_keeps_coordinator_evidence_and_cleanup_independent() {
    use crate::recovery::{
        clone::native::{post_process, seed, target_verify::input::request},
        fixture_control::source_pair::execution::tests::Fixture as PublishedFixture,
    };
    let fixture = Fixture::new();
    let published = PublishedFixture::new_in(&fixture.backend.join(".local-tests"));
    published.publication();
    let product = published.output.join("backend");
    let declared = json!({"path":product,"execution_source":published.binding()});
    let (selected, _) = request::execution_backend(
        &fixture.backend,
        Some(&declared),
        &request::SystemSourceObserver,
    )
    .unwrap();
    assert_eq!(selected, product);
    fixture.published_runtime_at(&product, None);
    let post = seed::history::post(&fixture.backend, &fixture.run, None, None, true).unwrap();
    let before = fs::read(fixture.runtime.join("runtime.json")).unwrap();
    let runtime =
        super::registration::cleanup_runtime(&fixture.backend, &fixture.run, &post).unwrap();
    assert_eq!(runtime["backend_root"], json!(product));
    assert_eq!(
        post_process::runtime::product_root(&fixture.backend, &runtime).unwrap(),
        product
    );
    fs::remove_file(fixture.temporary.path().join("source.json")).unwrap();
    fs::remove_file(fixture.target.join("initialized.json")).unwrap();
    assert_eq!(
        super::registration::cleanup_runtime(&fixture.backend, &fixture.run, &post).unwrap(),
        runtime,
    );
    let mut changed = runtime;
    changed["backend_root"] = json!(published.backend);
    fs::write(
        fixture.runtime.join("runtime.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    assert!(super::registration::cleanup_runtime(&fixture.backend, &fixture.run, &post).is_err());
    fs::write(fixture.runtime.join("runtime.json"), &before).unwrap();
    changed["backend_root"] = json!(fixture.backend.parent().unwrap());
    assert!(post_process::runtime::product_root(&fixture.backend, &changed).is_err());
    assert_eq!(
        fs::read(fixture.runtime.join("runtime.json")).unwrap(),
        before
    );
}

fn audited_product_build(root: &std::path::Path) -> (PathBuf, Value, Value) {
    use crate::recovery::clone::native::{bridge, source};
    let directory = root.join(".local-tests/product-root-build");
    fs::create_dir_all(&directory).unwrap();
    let inventory = source::capture(root).unwrap();
    let mut artifacts = serde_json::Map::new();
    for role in ["api", "worker"] {
        let path = directory.join(format!("{role}.bin"));
        fs::write(&path, format!("冻结{role}产物")).unwrap();
        let binding = common::binding(&path).unwrap();
        artifacts.insert(
            role.into(),
            json!({"executable":path,
            "command":bridge::build_command(role, 4).unwrap(),
            "bytes":binding["bytes"],"sha256":binding["sha256"]}),
        );
    }
    let build = json!({"format_version":2,"kind":"restore-backend-build",
        "sources":source::backend_domains(&inventory).unwrap(),
        "build":bridge::build_context::current(root).unwrap(),"artifacts":artifacts});
    let path = directory.join("build.json");
    let input = directory.join("inventory.json");
    let audit = directory.join("bridge.json");
    document::write_new(&path, &build).unwrap();
    document::write_new(&input, &source::inventory_value(&inventory)).unwrap();
    bridge::write(root, &path, &input, &audit).unwrap();
    let binding = common::binding(&audit).unwrap();
    (
        path,
        build,
        json!({"path":audit,"sha256":binding["sha256"]}),
    )
}

#[test]
fn v2_product_build_uses_registered_bridge_root_for_prepare_and_frozen_handoff() {
    use crate::recovery::{
        clone::native::{bridge::scope::Scope, post_process::runtime, seed},
        fixture_control::source_pair::execution::tests::Fixture as PublishedFixture,
    };
    let fixture = Fixture::new();
    let published = PublishedFixture::new_in(&fixture.backend.join(".local-tests"));
    published.publication();
    let product = published.output.join("backend");
    let target =
        json!({"execution_backend":{"path":product,"execution_source":published.binding()}});
    let (path, build, bridge) = audited_product_build(&product);
    assert!(build.get("backend_root").is_none());
    let before = fs::read(&path).unwrap();
    let scope = Scope::load(&fixture.backend, std::slice::from_ref(&bridge), &[]).unwrap();
    assert_eq!(
        runtime::selected_product_root(&fixture.backend, &target, &scope, &path, &build).unwrap(),
        product,
    );
    scope
        .product(
            &product,
            &path,
            &build,
            build["sources"]["full"]["source"]["worktree_fingerprint"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
    let same_root = Scope::load(&product, std::slice::from_ref(&bridge), &[]).unwrap();
    assert_eq!(
        runtime::selected_product_root(&product, &json!({}), &same_root, &path, &build).unwrap(),
        product,
    );
    assert!(
        runtime::selected_product_root(&fixture.backend, &json!({}), &scope, &path, &build,)
            .unwrap_err()
            .to_string()
            .contains("桥接根不一致")
    );
    let copied = path.with_file_name("unregistered.json");
    fs::write(&copied, &before).unwrap();
    assert!(scope.registered_product_root(&copied, &build).is_err());
    let mut changed = build;
    changed["build"]["jobs"] = json!("different");
    assert!(scope.registered_product_root(&path, &changed).is_err());
    let mut wrong_bridge =
        document::read(std::path::Path::new(bridge["path"].as_str().unwrap())).unwrap();
    wrong_bridge["backend_root"] = json!(fixture.backend);
    let wrong_path = path.with_file_name("wrong-root-bridge.json");
    document::write_new(&wrong_path, &wrong_bridge).unwrap();
    let wrong_binding = common::binding(&wrong_path).unwrap();
    assert!(
        Scope::load(
            &fixture.backend,
            &[json!({"path":wrong_path,
        "sha256":wrong_binding["sha256"]})],
            &[]
        )
        .is_err()
    );
    fixture.published_runtime_at(&product, Some(common::binding(&path).unwrap()));
    let post = seed::history::post(&fixture.backend, &fixture.run, None, None, true).unwrap();
    let handoff = document::read(&fixture.runtime.join("handoff.json")).unwrap();
    let contract = super::registration::runtime(
        &fixture.backend,
        &fixture.run,
        &post,
        &handoff["api_url"],
        &target,
        &scope,
    )
    .unwrap();
    assert_eq!(contract["backend_root"], json!(product));
    let (directory, binding) =
        super::guard::api_configuration_binding(&fixture.backend, &contract).unwrap();
    assert_eq!(binding["directory"], contract["api_config_dir"]);
    let file = common::binding(&directory.join("app.test.toml")).unwrap();
    assert_eq!(binding["files"]["app.test.toml"]["bytes"], file["bytes"]);
    assert_eq!(binding["files"]["app.test.toml"]["sha256"], file["sha256"]);
    let rejected = super::guard::api_configuration_binding(&product, &contract).unwrap_err();
    let diagnosed =
        super::diagnostic::failure(super::diagnostic::Phase::BeforeStart, rejected, None);
    assert!(
        diagnosed
            .to_string()
            .contains("配置目录不属于已登记配置归属根")
    );
    assert_stopped_roles_preserve_frozen_runtime(&fixture, &product);
    assert!(
        super::registration::runtime(
            &fixture.backend,
            &fixture.run,
            &post,
            &handoff["api_url"],
            &json!({}),
            &scope,
        )
        .is_err()
    );
    fs::remove_file(&path).unwrap();
    assert_eq!(
        super::registration::cleanup_runtime(&fixture.backend, &fixture.run, &post).unwrap(),
        contract,
    );
    assert!(!path.exists());
    assert_eq!(fs::read(copied).unwrap(), before);
}

fn assert_stopped_roles_preserve_frozen_runtime(fixture: &Fixture, product: &std::path::Path) {
    let canonical =
        crate::recovery::fixture_control::portable_path(&product.canonicalize().unwrap());
    let files = [
        "runtime.json",
        "binaries.json",
        "handoff.json",
        "config/app.test.toml",
        "worker-config/app.test.toml",
    ]
    .map(|name| fixture.runtime.join(name));
    let frozen = files
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();
    for role in [
        crate::recovery::full_stack::worker::role::Role::Api,
        crate::recovery::full_stack::worker::role::Role::Worker,
    ] {
        let observed = crate::recovery::full_stack::worker::controller::observe(
            role,
            &canonical,
            &fixture.runtime,
        )
        .unwrap();
        assert_eq!(observed, json!({"state":"stopped","identity":null}));
        assert_eq!(
            files
                .iter()
                .map(|path| fs::read(path).unwrap())
                .collect::<Vec<_>>(),
            frozen
        );
    }
}
