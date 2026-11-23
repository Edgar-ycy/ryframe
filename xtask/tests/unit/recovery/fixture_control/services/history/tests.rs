use std::{fs, path::PathBuf};

use super::{
    evidence::{binding, read},
    *,
};
use crate::recovery::clone::native::{digest, json as json_io};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Fixture {
    _temporary: tempfile::TempDir,
    run: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        fs::create_dir_all(&workspace).unwrap();
        let temporary = tempfile::tempdir_in(&workspace).unwrap();
        let run = super::super::super::portable_path(&fs::canonicalize(temporary.path()).unwrap());
        json_io::write_new(&run.join("manifest.json"), &json!({"fixture":true})).unwrap();
        state::initialize(&run).unwrap();
        let fixture = Self {
            _temporary: temporary,
            run,
        };
        for (stage, mode) in INITIAL {
            let (number, _) = state::begin(&fixture.run, stage, mode, &json!({})).unwrap();
            state::finish(&fixture.run, number, Some(&json!({"service":stage})), None).unwrap();
        }
        fixture
    }
    fn load(&self) -> Value {
        state::load(&self.run, true).unwrap()
    }
    fn history(&self) -> Value {
        validate(&self.run, &self.load(), true, true).unwrap()
    }
    fn stage(&self, mode: &str, proof: Option<&Value>) -> u64 {
        let sources = json!({"manifest":binding(&self.run.join("manifest.json")).unwrap(),"state_before":binding(&self.run.join("state.json")).unwrap()});
        let (number, _) = state::begin(&self.run, STAGE, mode, &sources).unwrap();
        state::finish(
            &self.run,
            number,
            proof,
            proof.is_none().then_some("InjectedFailure"),
        )
        .unwrap();
        number
    }
    fn close(&self) -> Value {
        let root = self.run.join("lifecycle-0004");
        fs::create_dir(&root).unwrap();
        fs::create_dir(self.run.join("rustfs")).unwrap();
        let tree = json!({"format_version":2,"runtime_directory":self.run.join("rustfs"),"role":"rustfs",
            "scope_id":"fixture-test","operation_id":"b".repeat(32),"supervisor":{"pid":100},"monitor":{"pid":101},"process":{"pid":102}});
        let completion = handoff_completion(&tree);
        json_io::write_new(
            &root.join("stopped.json"),
            &json!({"status":"redis_process_stopped","alive":false,"resources_deleted":false}),
        )
        .unwrap();
        json_io::write_new(
            &root.join("rustfs-stopped.json"),
            &json!({"state":"stopped","tree":tree,"completion":completion}),
        )
        .unwrap();
        json!({"kind":"reference-fixture-service-lifecycle","operation":"close","run":self.run,
            "remote_writes":0,"resources_deleted":false,"status":"services_closed","services":{"redis":"stopped","rustfs":"stopped"},
            "evidence":{"redis":binding(&root.join("stopped.json")).unwrap(),"rustfs":binding(&root.join("rustfs-stopped.json")).unwrap()}})
    }
}

fn handoff_completion(tree: &Value) -> Value {
    let directory = Path::new(tree["runtime_directory"].as_str().unwrap());
    let operation = tree["operation_id"].as_str().unwrap();
    let path = directory.join(format!("rustfs-members-{operation}-stopped.json"));
    json_io::write_new(&path, &json!({"status":"stopped","error_type":null,"operation_id":operation,
        "directory":directory,"role":"rustfs","scope_id":tree["scope_id"],"supervisor":tree["supervisor"],
        "monitor":tree["monitor"],"members":[tree["supervisor"],tree["monitor"],tree["process"]]})).unwrap();
    binding(&path).unwrap()
}

#[test]
fn initial_order_readiness_and_unsettled_are_strict() {
    let fixture = Fixture::new();
    assert_eq!(fixture.history()["initial_complete"], true);
    for index in 0..3 {
        let mut value = fixture.load();
        value["attempts"][index]["stage"] = json!("unknown");
        assert!(validate(&fixture.run, &value, true, true).is_err());
        value = fixture.load();
        value["attempts"][index]["status"] = json!("failed");
        assert!(validate(&fixture.run, &value, true, true).is_err());
    }
    fixture.stage("close", None);
    assert!(validate(&fixture.run, &fixture.load(), true, true).is_err());
    let history = validate(&fixture.run, &fixture.load(), true, false).unwrap();
    assert_eq!(history["unsettled"], json!([4]));
    assert!(reconcilable(&fixture.load(), &history, "close").is_some());
    assert!(reconcilable(&fixture.load(), &history, "restart").is_none());
}

#[test]
fn close_preserves_prefix_is_readonly_and_rejects_tampered_members() {
    let fixture = Fixture::new();
    let proof = fixture.close();
    let before = binding(&fixture.run.join("state.json")).unwrap();
    fixture.stage("close", Some(&proof));
    let snapshot = fs::read(fixture.run.join("state.json")).unwrap();
    assert_eq!(fixture.history()["closed"], true);
    assert_eq!(prefix::count(&fixture.run, &before).unwrap(), 3);
    assert_eq!(fs::read(fixture.run.join("state.json")).unwrap(), snapshot);
    let completion = fixture.run.join(format!(
        "rustfs/rustfs-members-{}-stopped.json",
        "b".repeat(32)
    ));
    fs::write(completion, b"tampered").unwrap();
    assert!(validate(&fixture.run, &fixture.load(), true, true).is_err());
}

#[test]
fn replay_checks_a_historical_view_and_rechecks_all_current_results() {
    let fixture = Fixture::new();
    let state_path = fixture.run.join("state.json");
    let before = binding(&state_path).unwrap();
    fixture.stage("close", Some(&fixture.close()));
    let historical = fixture.load();
    fixture.stage("restart", None);
    assert_eq!(
        validate(&fixture.run, &historical, true, true).unwrap()["closed"],
        true
    );

    let replay = prefix::Snapshot::open(&fixture.run).unwrap();
    let descriptor = prefix::Descriptor::read(&fixture.run, &before).unwrap();
    assert_eq!(replay.count(descriptor).unwrap(), 3);
    let mut different = before;
    different["sha256"] = json!("0".repeat(64));
    let descriptor = prefix::Descriptor::read(&fixture.run, &different).unwrap();
    assert!(replay.count(descriptor).is_err());
    replay.check_end().unwrap();

    #[cfg(windows)]
    assert!(fs::OpenOptions::new().write(true).open(state_path).is_err());
    let first_result = state::result_path(&fixture.run, 1);
    let mut raw = fs::read(&first_result).unwrap();
    raw.push(b'\n');
    fs::write(first_result, raw).unwrap();
    assert!(replay.check_end().is_err());
}

#[test]
fn rustfs_handoff_history_keeps_original_redis_and_binds_immutable_close() {
    let fixture = Fixture::new();
    let copy = Fixture::new();
    let close = json!({"status":"seed_runtime_closed","outbox_drained":true});
    let (seed, _) = state::begin(&copy.run, "seed-runtime", "close", &json!({})).unwrap();
    state::finish(&copy.run, seed, Some(&close), None).unwrap();
    let seed_path = state::result_path(&copy.run, seed);
    let close_binding = binding(&seed_path).unwrap();
    let copy_state = binding(&copy.run.join("state.json")).unwrap();

    let template = fixture.close();
    let rustfs_path = Path::new(template["evidence"]["rustfs"]["path"].as_str().unwrap());
    let mut rustfs = read(rustfs_path).unwrap();
    let identity = json!({"pid":103});
    rustfs["data_directory"] = json!({"path":fixture.run.join("data"),"device":1,"inode":2});
    rustfs["terminated"] = json!(true);
    rustfs["redis"] =
        json!({"process":identity,"run_id":"original","markers_sha256":"c".repeat(64)});
    json_io::write_replace(rustfs_path, &rustfs).unwrap();
    for (name, value) in [
        ("rustfs-tree.json", rustfs["tree"].clone()),
        (
            "request.json",
            json!({"data_directory":rustfs["data_directory"]}),
        ),
    ] {
        json_io::write_new(&fixture.run.join("rustfs").join(name), &value).unwrap();
    }
    let redis = fixture.run.join("redis/runtime.json");
    fs::create_dir(redis.parent().unwrap()).unwrap();
    json_io::write_new(
        &redis,
        &json!({"format_version":3,"process":identity,
        "redis":{"run_id":"original"}}),
    )
    .unwrap();
    let sources = json!({"manifest":binding(&fixture.run.join("manifest.json")).unwrap(),
        "state_before":binding(&fixture.run.join("state.json")).unwrap(),
        "seed_close":close_binding,"copy_state":copy_state});
    let owner =
        crate::recovery::clone::native::lifecycle::lock::RunLock::claim(&fixture.run).unwrap();
    let (number, attempt) = state::begin(&fixture.run, STAGE, "handoff-rustfs", &sources).unwrap();
    state::bind_controller(&fixture.run, number, &owner.owner, &attempt).unwrap();
    let result = json!({"kind":"reference-fixture-service-lifecycle","operation":"handoff-rustfs",
        "run":fixture.run,"remote_writes":0,"resources_deleted":false,"status":"rustfs_handed_off",
        "services":{"redis":"running","rustfs":"handed-off"},"seed_close":close_binding,"copy_state":copy_state,
        "controller":binding(&state::controller_path(&fixture.run,number)).unwrap(),
        "evidence":{"rustfs":binding(rustfs_path).unwrap(),"redis":binding(&redis).unwrap()}});
    state::finish(&fixture.run, number, Some(&result), None).unwrap();
    owner.release().unwrap();
    let descriptor = binding(&state::result_path(&fixture.run, number)).unwrap();
    assert_eq!(fixture.history()["rustfs_handoff"], descriptor);
    assert!(fixture.history()["restart_predecessor"].is_null());

    let (restart, _) = state::begin(&copy.run, "storage-target", "restart", &json!({})).unwrap();
    state::finish(
        &copy.run,
        restart,
        Some(&json!({"status":"storage_restarted"})),
        None,
    )
    .unwrap();
    assert_eq!(fixture.history()["rustfs_handoff"], descriptor);
    json_io::write_replace(&seed_path, &json!({"status":"changed"})).unwrap();
    assert!(validate(&fixture.run, &fixture.load(), true, true).is_err());
}

#[cfg(windows)]
mod successor_handoff {
    use super::*;

    #[test]
    fn rustfs_handoff_history_requires_the_validated_successor_tree_and_redis() {
        let fixture = Fixture::new();
        let copy = Fixture::new();
        let original_redis = windows_redis_origin(&fixture);
        let (copy_manifest, previous) = fresh_storage_origin(&fixture, &copy);
        let (seed, _) = state::begin(&copy.run, "seed-runtime", "close", &json!({})).unwrap();
        state::finish(
            &copy.run,
            seed,
            Some(&json!({"status":"seed_runtime_closed"})),
            None,
        )
        .unwrap();
        let seed_close = binding(&state::result_path(&copy.run, seed)).unwrap();
        let copy_state = binding(&copy.run.join("state.json")).unwrap();
        let original = fixture.close();
        let original_tree = read(Path::new(
            original["evidence"]["rustfs"]["path"].as_str().unwrap(),
        ))
        .unwrap()["tree"]
            .clone();
        let rustfs = json!({"data_directory":{"path":fixture.run.join("data"),"device":1,"inode":2},
        "scope_id":"fixture-test","api_url":"http://127.0.0.1:21951","console_url":"http://127.0.0.1:21952",
        "credential_files":{},"executable":{"path":"D:/tools/rustfs.exe","sha256":"a".repeat(64)}});
        let original_path = fixture.run.join("rustfs").join("rustfs-tree.json");
        json_io::write_new(&original_path, &original_tree).unwrap();
        json_io::write_new(&previous, &json!({"identity":original_tree["process"]})).unwrap();
        json_io::write_new(&fixture.run.join("rustfs/request.json"), &rustfs).unwrap();
        let predecessor = external_generation(&fixture, &original_path, &original_redis);
        let (tree, redis) = successor_generation(&fixture, &rustfs, &original_redis, &predecessor);
        let evidence = |tree: &Value, redis_binding: &Value, completion: &Value| {
            let cache = read(Path::new(redis_binding["path"].as_str().unwrap())).unwrap();
            json!({"tree":tree,"state":"stopped","terminated":true,"completion":completion,
            "data_directory":rustfs["data_directory"],"redis":{"process":cache["process"],
            "run_id":cache["redis"]["run_id"],"markers_sha256":"c".repeat(64)}})
        };
        let completion = handoff_completion(&tree);
        let stopped = evidence(&tree, &redis, &completion);
        let sources = json!({"manifest":binding(&fixture.run.join("manifest.json")).unwrap(),
        "state_before":binding(&fixture.run.join("state.json")).unwrap(),"seed_close":seed_close,"copy_state":copy_state});
        let owner =
            crate::recovery::clone::native::lifecycle::lock::RunLock::claim(&fixture.run).unwrap();
        let (number, attempt) =
            state::begin(&fixture.run, STAGE, "handoff-rustfs", &sources).unwrap();
        state::bind_controller(&fixture.run, number, &owner.owner, &attempt).unwrap();
        let output = fixture.run.join(format!("lifecycle-{number:04}"));
        fs::create_dir(&output).unwrap();
        let proof = output.join("rustfs-stopped.json");
        json_io::write_new(&proof, &stopped).unwrap();
        let mut result = json!({"kind":"reference-fixture-service-lifecycle","operation":"handoff-rustfs",
        "run":fixture.run,"remote_writes":0,"resources_deleted":false,"status":"rustfs_handed_off",
        "services":{"redis":"running","rustfs":"handed-off"},"seed_close":seed_close,"copy_state":copy_state,
        "controller":binding(&state::controller_path(&fixture.run,number)).unwrap(),
        "evidence":{"rustfs":binding(&proof).unwrap(),"redis":redis}});
        state::finish(&fixture.run, number, Some(&result), None).unwrap();
        owner.release().unwrap();
        let result_path = state::result_path(&fixture.run, number);
        let descriptor = binding(&result_path).unwrap();
        assert_eq!(fixture.history()["rustfs_handoff"], descriptor);
        assert_storage_reader(
            &copy,
            &copy_manifest,
            &previous,
            &descriptor,
            &rustfs,
            &original_tree["process"],
        );

        let mut historic = fixture.load();
        let original_completion = read(Path::new(
            original["evidence"]["rustfs"]["path"].as_str().unwrap(),
        ))
        .unwrap()["completion"]
            .clone();
        for (tree, cache, completed) in [
            (&tree, &original_redis, &completion),
            (&original_tree, &redis, &original_completion),
        ] {
            json_io::write_replace(&proof, &evidence(tree, cache, completed)).unwrap();
            result["evidence"] = json!({"rustfs":binding(&proof).unwrap(),"redis":cache});
            json_io::write_replace(&result_path, &result).unwrap();
            historic["attempts"][(number - 1) as usize]["result"] = binding(&result_path).unwrap();
            assert!(validate(&fixture.run, &historic, true, true).is_err());
        }
    }

    fn fresh_storage_origin(fixture: &Fixture, copy: &Fixture) -> (Value, PathBuf) {
        let review = fixture.run.join("review.json");
        let value = json!({"scopes":{"seed":{}}});
        json_io::write_new(&review, &value).unwrap();
        let review = binding(&review).unwrap();
        let manifest_path = fixture.run.join("manifest.json");
        let manifest = json!({"format_version":1,"kind":"reference-fixture-service-run",
        "review":review,"bootstrap":{},"execution_backend":fixture.run,"scope_id":"fixture-test",
        "data_directory_was_empty":true});
        json_io::write_replace(&manifest_path, &manifest).unwrap();
        let ledger = fixture.run.join("state.json");
        let mut saved = read(&ledger).unwrap();
        saved["manifest"] = binding(&manifest_path).unwrap();
        json_io::write_replace(&ledger, &saved).unwrap();
        let workspace = copy.run.join("fresh");
        let target = workspace.join("target");
        fs::create_dir_all(&target).unwrap();
        let original_process = fixture.run.join("rustfs/process.json");
        let mut source_review = review;
        source_review["canonical_sha256"] = json!(digest::plan_hash(&value).unwrap());
        let request = json!({"side":"seed","review":source_review});
        let path = target.join("request.json");
        json_io::write_new(&path, &request).unwrap();
        let prepare = target.join("prepare.json");
        json_io::write_new(
            &prepare,
            &json!({"request":binding(&path).unwrap(),
        "request_sha256":digest::plan_hash(&request).unwrap()}),
        )
        .unwrap();
        let initialized = target.join("initialized.json");
        json_io::write_new(
            &initialized,
            &json!({"status":"fresh_target_initialized",
        "prepare_sha256":binding(&prepare).unwrap()["sha256"]}),
        )
        .unwrap();
        json_io::write_new(&workspace.join("registration.json"), &json!({
        "format_version":1,"kind":"devex-clone-fresh-target-registration","target_directory":target,
        "environment":{"fixture":true},"storage_run":{"path":fixture.run,
        "manifest":binding(&manifest_path).unwrap(),"state":binding(&ledger).unwrap()}
    })).unwrap();
        (
            json!({"initialized":binding(&initialized).unwrap(),"target_environment":{"fixture":true}}),
            original_process,
        )
    }

    fn assert_storage_reader(
        copy: &Fixture,
        manifest: &Value,
        previous: &Path,
        receipt: &Value,
        rustfs: &Value,
        identity: &Value,
    ) {
        use crate::recovery::clone::native::lifecycle::manifest::fixture_storage::rustfs_handoff;
        let backend = crate::workspace::root_dir();
        let mut request = rustfs.clone();
        request["previous"] =
            json!({"identity":identity,"process_receipt":binding(previous).unwrap()});
        let (restart, _) =
            state::begin(&copy.run, "storage-target", "restart", &json!({})).unwrap();
        state::finish(
            &copy.run,
            restart,
            Some(&json!({"status":"storage_restarted","fixture_handoff":receipt})),
            None,
        )
        .unwrap();
        let observed = rustfs_handoff(&backend, &copy.run, manifest, &request, false)
            .unwrap()
            .unwrap();
        assert_eq!(&observed.binding, receipt);
        request["data_directory"]["inode"] = json!(3);
        assert!(rustfs_handoff(&backend, &copy.run, manifest, &request, false).is_err());
    }

    fn windows_redis_origin(fixture: &Fixture) -> Value {
        let directory = fixture.run.join("redis");
        fs::create_dir(&directory).unwrap();
        let tool = json!({"platform":"windows","path":"D:/tools/redis-server.exe","sha256":"a".repeat(64),
        "version":"Redis server v=7.4.11 bits=64","dlls":{"cygwin1.dll":"b".repeat(64)}});
        let request = json!({"format_version":3,"tool":tool,"configuration":{"path":directory.join("redis.conf")},
        "previous_identity":null,"previous_run_id":null,"port":16390});
        json_io::write_new(&directory.join("request.json"), &request).unwrap();
        let runtime = windows_redis_runtime(&directory, &request, 200, 'a');
        let result = state::result_path(&fixture.run, 2);
        json_io::write_replace(
        &result,
        &json!({"service":"redis","request_sha256":runtime["request_sha256"],"runtime":runtime}),
    )
    .unwrap();
        let state_path = fixture.run.join("state.json");
        let mut state = read(&state_path).unwrap();
        state["attempts"][1]["result"] = binding(&result).unwrap();
        json_io::write_replace(&state_path, &state).unwrap();
        binding(&directory.join("runtime.json")).unwrap()
    }

    fn windows_redis_runtime(directory: &Path, request: &Value, pid: u32, digit: char) -> Value {
        let process =
            json!({"pid":pid,"started":pid.to_string(),"executable":request["tool"]["path"]});
        let runtime = json!({"format_version":3,"output":directory,"request_sha256":digest::plan_hash(request).unwrap(),
        "supervisor":{"pid":pid+20,"started":(pid+20).to_string(),"executable":request["tool"]["path"]},
        "process":process,"redis":{"platform":"windows","tool":request["tool"],"pid":pid,"port":request["port"],
        "started":process["started"],"run_id":digit.to_string().repeat(40)}});
        json_io::write_new(&directory.join("runtime.json"), &runtime).unwrap();
        runtime
    }

    fn external_generation(fixture: &Fixture, tree: &Path, redis: &Value) -> Value {
        let number = 4;
        let output = fixture.run.join(format!("lifecycle-{number:04}"));
        let evidence = output.join("external-termination.json");
        let ready = tree.with_file_name("rustfs-test-ready.json");
        json_io::write_new(&ready, &json!({"ready":true})).unwrap();
        let owner = binding(&fixture.run.join("state.json")).unwrap();
        let cache = read(Path::new(redis["path"].as_str().unwrap())).unwrap();
        json_io::write_new(&evidence, &json!({"format_version":2,"kind":"reference-fixture-external-termination",
        "run":fixture.run,"owner":owner,"normal_shutdown_proof":null,"remote_writes":0,"resources_deleted":false,
        "observation":{"status":"external-termination-unreconciled","normal_shutdown_proof":null,
        "identities":{"supervisor":"missing","monitor":"missing","process":"missing"},
        "evidence":{"rustfs_tree":binding(tree).unwrap(),"redis_runtime":redis,"rustfs_monitor_ready":binding(&ready).unwrap(),
        "redis_external":{"kind":"windows-redis-external-termination","runtime":redis,"supervisor":cache["supervisor"],
            "process":cache["process"],"port":cache["redis"]["port"],"normal_job_exit_proof":null}}}})).unwrap();
        assert_eq!(
        fixture.stage("recover", Some(&json!({"kind":"reference-fixture-service-lifecycle","operation":"recover",
            "run":fixture.run,"remote_writes":0,"resources_deleted":false,"status":"external_termination_reconciled",
            "owner":owner,"evidence":binding(&evidence).unwrap()}))),
        number,
    );
        binding(&state::result_path(&fixture.run, number)).unwrap()
    }

    fn successor_generation(
        fixture: &Fixture,
        rustfs: &Value,
        redis: &Value,
        predecessor: &Value,
    ) -> (Value, Value) {
        let previous = fixture.history()["active_generation"].clone();
        let sources = json!({"manifest":binding(&fixture.run.join("manifest.json")).unwrap(),
        "state_before":binding(&fixture.run.join("state.json")).unwrap(),"restart_predecessor":predecessor});
        let owner =
            crate::recovery::clone::native::lifecycle::lock::RunLock::claim(&fixture.run).unwrap();
        let (number, attempt) = state::begin(&fixture.run, STAGE, "restart", &sources).unwrap();
        state::bind_controller(&fixture.run, number, &owner.owner, &attempt).unwrap();
        let controller = binding(&state::controller_path(&fixture.run, number)).unwrap();
        let root = fixture.run.join(format!("lifecycle-{number:04}"));
        let storage = root.join("rustfs");
        let cache = root.join("redis");
        fs::create_dir_all(&storage).unwrap();
        fs::create_dir(&cache).unwrap();
        let tree = json!({"format_version":2,"runtime_directory":storage,"role":"rustfs",
        "scope_id":"fixture-test","operation_id":"d".repeat(32),"supervisor":{"pid":210},"monitor":{"pid":211},"process":{"pid":212}});
        for (name, value) in [
            ("request.json", rustfs.clone()),
            ("rustfs-tree.json", tree.clone()),
            ("process.json", json!({"identity":tree["process"]})),
            ("launch.json", json!({"fixture":true})),
        ] {
            json_io::write_new(&storage.join(name), &value).unwrap();
        }
        let original = read(Path::new(redis["path"].as_str().unwrap())).unwrap();
        let mut request = read(&fixture.run.join("redis/request.json")).unwrap();
        request["previous_identity"] = original["process"].clone();
        request["previous_run_id"] = original["redis"]["run_id"].clone();
        json_io::write_new(&cache.join("request.json"), &request).unwrap();
        let runtime = windows_redis_runtime(&cache, &request, 201, 'b');
        let generation = root.join("restart.json");
        json_io::write_new(&generation, &json!({"format_version":1,"kind":"reference-fixture-service-generation",
        "run":fixture.run,"attempt":number,"owner":sources["state_before"],"predecessor":predecessor,
        "previous_generation":previous,"controller":controller,"services":{"redis":"running","rustfs":"running"},
        "remote_writes":0,"resources_deleted":false,
        "rustfs":{"request":binding(&storage.join("request.json")).unwrap(),"storage":{
            "identity":tree["process"],"sha256":rustfs["executable"]["sha256"],
            "process_receipt":binding(&storage.join("process.json")).unwrap(),
            "launch_receipt":binding(&storage.join("launch.json")).unwrap(),"tree":binding(&storage.join("rustfs-tree.json")).unwrap()}},
        "redis":{"request":binding(&cache.join("request.json")).unwrap(),"runtime":runtime}})).unwrap();
        state::finish(
            &fixture.run,
            number,
            Some(&json!({"kind":"reference-fixture-service-lifecycle",
        "operation":"restart","run":fixture.run,"remote_writes":0,"resources_deleted":false,
        "status":"services_restarted","services":{"rustfs":"running","redis":"running"},
            "generation":binding(&generation).unwrap(),"controller":controller})),
            None,
        )
        .unwrap();
        owner.release().unwrap();
        assert_eq!(
            fixture.history()["active_generation"],
            binding(&state::result_path(&fixture.run, number)).unwrap()
        );
        (tree, binding(&cache.join("runtime.json")).unwrap())
    }
}

#[test]
fn close_rejects_duplicate_unknown_stage_and_wrong_predecessor() {
    let fixture = Fixture::new();
    let proof = fixture.close();
    fixture.stage("close", Some(&proof));
    let mut value = fixture.load();
    value["attempts"][3]["mode"] = json!("unknown");
    assert!(validate(&fixture.run, &value, true, true).is_err());
    fixture.stage("close", Some(&proof));
    assert!(validate(&fixture.run, &fixture.load(), true, true).is_err());
}

#[test]
fn history_rejects_nonzero_writes_resource_deletion_and_forged_run() {
    for (key, value) in [
        ("remote_writes", json!(1)),
        ("resources_deleted", json!(true)),
        ("run", json!("elsewhere")),
    ] {
        let fixture = Fixture::new();
        let mut proof = fixture.close();
        proof[key] = value;
        fixture.stage("close", Some(&proof));
        assert!(validate(&fixture.run, &fixture.load(), true, true).is_err());
    }
}

#[test]
fn prefix_preserves_all_writers_unicode_and_crlf() {
    let fixture = Fixture::new();
    let mut initial = fixture.load();
    initial["attempts"][0]["sources"] = json!({"中文":"🌏\u{7f}"});
    json_io::write_replace(&fixture.run.join("state.json"), &initial).unwrap();
    let raw = serde_json::to_string(&initial).unwrap();
    let descriptors = state::prefix::serializations(&raw).into_iter().map(|raw| json!({"path":fixture.run.join("state.json"),"bytes":raw.len(),"sha256":hex::encode(Sha256::digest(&raw))})).collect::<Vec<_>>();
    fixture.stage("close", None);
    for descriptor in descriptors {
        assert_eq!(prefix::count(&fixture.run, &descriptor).unwrap(), 3);
        assert_eq!(
            state::historical(&fixture.run, &descriptor, true).unwrap()["historical_attempts"],
            3
        );
    }
    let mut descriptor = binding(&fixture.run.join("state.json")).unwrap();
    descriptor["sha256"] = json!("a".repeat(64));
    assert!(prefix::count(&fixture.run, &descriptor).is_err());
    assert!(state::historical(&fixture.run, &descriptor, true).is_err());

    let fixture = Fixture::new();
    let mut initial = fixture.load();
    initial["attempts"][0]["sources"] = serde_json::from_str(
        r#"{"numbers":[18446744073709551616,-9223372036854775809,12345678901234567890.123456789,1e400,-0.0,1.25],"中文":"🌏"}"#,
    ).unwrap();
    json_io::write_replace(&fixture.run.join("state.json"), &initial).unwrap();
    let native = serde_json::to_string_pretty(&initial).unwrap() + "\n";
    let writers = state::prefix::serializations(&serde_json::to_string(&initial).unwrap());
    assert_eq!(writers[0], native.as_bytes());
    assert_eq!(writers[3], native.replace('\n', "\r\n").as_bytes());
    let descriptors = writers.into_iter().map(|raw| json!({
        "path":fixture.run.join("state.json"),"bytes":raw.len(),"sha256":hex::encode(Sha256::digest(&raw))
    })).collect::<Vec<_>>();
    fixture.stage("close", None);
    let current = fs::read(fixture.run.join("state.json")).unwrap();
    for descriptor in descriptors {
        assert_eq!(prefix::count(&fixture.run, &descriptor).unwrap(), 3);
        assert_eq!(
            state::historical(&fixture.run, &descriptor, true).unwrap()["historical_attempts"],
            3
        );
    }
    assert_eq!(fs::read(fixture.run.join("state.json")).unwrap(), current);
}

#[test]
fn prefix_serialization_matches_python_escaping() {
    let variants = state::prefix::serializations(r#"{"z":"\u007f中文🌏","a":[]}"#);
    assert_eq!(
        String::from_utf8(variants[0].clone()).unwrap(),
        "{\n  \"a\": [],\n  \"z\": \"\u{7f}中文🌏\"\n}\n"
    );
    assert_eq!(
        String::from_utf8(variants[1].clone()).unwrap(),
        "{\n  \"z\": \"\\u007f\\u4e2d\\u6587\\ud83c\\udf0f\",\n  \"a\": []\n}\n"
    );
}

#[test]
fn protocol_rejects_unknown_duplicate_and_oversized_without_echoing_secrets() {
    let secret = "private-credential-marker";
    let inputs = [
        format!(r#"{{"format_version":1,"operation":"{secret}","backend":"x","arguments":{{}}}}"#)
            .into_bytes(),
        format!(r#"{{"format_version":1,"format_version":2,"password":"{secret}"}}"#).into_bytes(),
        vec![b'X'; 4 * 1024 * 1024 + 1],
    ];
    for input in inputs {
        let (code, response) = protocol::execute(&input);
        assert_eq!(code, 2);
        assert!(!response.to_string().contains(secret));
    }
}

#[test]
fn manifest_rejects_unknown_fields_and_false_empty_directory() {
    let mut manifest = json!({"format_version":1,"kind":"reference-fixture-service-run","review":{},"bootstrap":{},
        "execution_backend":"backend","scope_id":"fixture","data_directory_was_empty":true});
    validate_manifest(&manifest).unwrap();
    manifest["data_directory_was_empty"] = json!(false);
    assert!(validate_manifest(&manifest).is_err());
    manifest["data_directory_was_empty"] = json!(true);
    manifest["extra"] = json!(true);
    assert!(validate_manifest(&manifest).is_err());
}

#[test]
fn reconcilable_restart_requires_only_one_last_failed_restart_and_external_owner() {
    let value = json!({"attempts":[{"number":1,"stage":STAGE,"mode":"restart","status":"failed"}]});
    let history = json!({"closed":false,"external_recovery":{"path":"recovery"},"unsettled":[1]});
    assert!(reconcilable(&value, &history, "restart").is_some());
    for (key, replacement) in [
        ("closed", json!(true)),
        ("external_recovery", Value::Null),
        ("unsettled", json!([1, 2])),
    ] {
        let mut invalid = history.clone();
        invalid[key] = replacement;
        assert!(reconcilable(&value, &invalid, "restart").is_none());
    }
}

#[test]
fn bound_evidence_rejects_path_escape_and_content_drift() {
    let fixture = Fixture::new();
    let mut descriptor = binding(&fixture.run.join("manifest.json")).unwrap();
    descriptor["path"] = json!(fixture.run.join("../outside.json"));
    assert!(evidence::bound(&fixture.run, &descriptor).is_err());
    let descriptor = binding(&fixture.run.join("manifest.json")).unwrap();
    fs::write(fixture.run.join("manifest.json"), b"{}").unwrap();
    assert!(evidence::bound(&fixture.run, &descriptor).is_err());
    assert_eq!(read(&fixture.run.join("manifest.json")).unwrap(), json!({}));
    assert_eq!(
        digest::file(&fixture.run.join("manifest.json"))
            .unwrap()
            .bytes,
        2
    );
}

#[test]
fn failed_close_selection_is_native_without_python_evidence_bridge() {
    let state = json!({"attempts":[{"number":1,"stage":"fixture-services","mode":"close","status":"failed"}]});
    let history = json!({"closed":false,"external_recovery":null,"unsettled":[1]});
    assert_eq!(
        super::reconcilable(&state, &history, "close").unwrap()["number"],
        1
    );
    assert!(
        super::reconcilable(
            &state,
            &json!({"closed":false,"external_recovery":null,"unsettled":[]}),
            "close"
        )
        .is_none()
    );
}
