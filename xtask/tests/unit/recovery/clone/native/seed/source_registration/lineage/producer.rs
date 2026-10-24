use super::*;
use crate::recovery::{clone::native::lifecycle::state, fixture_control::portable_path};
use std::{fs, path::PathBuf};

struct Fixture {
    _root: tempfile::TempDir,
    backend: PathBuf,
    run: PathBuf,
    output: PathBuf,
    registered: Value,
    tool: Value,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let backend = portable_path(&fs::canonicalize(root.path()).unwrap());
        let run = backend.join(".local-tests/run");
        let output = run.join("seed-runtime/attempt-0001");
        fs::create_dir_all(&output).unwrap();
        fs::write(run.join("manifest.json"), b"{}").unwrap();
        let executable = backend.join("tool.exe");
        fs::write(&executable, b"historical tool").unwrap();
        let tool = json!({"path":executable,"sha256":digest::file(&executable).unwrap().sha256});
        let source = command::source(&backend, "identity-apply").unwrap();
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "// 历史来源\n").unwrap();
        let descriptor = common::binding(&source).unwrap();
        let request = json!({"identity_tool":tool,"identity_plan":{"path":backend.join("plan.json"),"sha256":"a".repeat(64)},"identity_state":run.join("identity-state.json")});
        json_io::write_new(&run.join("seed-runtime.json"), &request).unwrap();
        let snapshot = json!({"snapshot":{"head":"a".repeat(40),"patch_sha256":"b".repeat(64),
            "clean":false,"files":[{"path":"xtask/src/data/performance_identities/producer.rs","sha256":descriptor["sha256"]},{"path":"z-history.txt","sha256":"e".repeat(64)}]},
            "worktree_fingerprint":format!("sha256:{}","c".repeat(64)),
            "fingerprints":{"product":{"files":1,"sha256":"d".repeat(64)},"test_tools":{"files":0,"sha256":"e".repeat(64)},"support":{"files":0,"sha256":"f".repeat(64)}}});
        let attempt = json!({"number":1,"stage":"seed-runtime","mode":"identities-apply","started_at":"start","finished_at":"finish","status":"passed","sources":snapshot,"result":null,"error_type":null});
        let mut running = attempt.clone();
        running["status"] = json!("running");
        running["finished_at"] = Value::Null;
        let identity = json!({"pid":42,"started":"123","executable":executable});
        let controller = json!({"format_version":1,"kind":"devex-stage-controller","attempt":1,
            "attempt_sha256":digest::plan_hash(&running).unwrap(),"owner":{"format_version":1,"identity":identity,"directory":run,"manifest_sha256":common::binding(&run.join("manifest.json")).unwrap()["sha256"]}});
        let controller_path = run.join("controller-0001.json");
        json_io::write_new(&controller_path, &controller).unwrap();
        let registered =
            json!({"attempt":attempt,"controller":common::binding(&controller_path).unwrap()});
        let plan = command::build(&json!({"backend":backend,"run_dir":run,"attempt":1,"kind":"identity-apply","output":output,"tool":tool,"registration":request,"configuration":null})).unwrap();
        let launch = json!({"format_version":1,"kind":"devex-post-producer-launch","run_manifest":common::binding(&run.join("manifest.json")).unwrap(),
            "registration":common::binding(&run.join("seed-runtime.json")).unwrap(),"controller":registered["controller"],"attempt":1,"producer_kind":"identity-apply",
            "command":plan["argv"],"tool":tool,"source":descriptor});
        json_io::write_new(&output.join("session-launch.json"), &launch).unwrap();
        json_io::write_new(&output.join("session-process.json"), &json!({"format_version":1,"kind":"devex-post-producer",
            "launch":common::binding(&output.join("session-launch.json")).unwrap(),"identity":identity,"identity_lock_owner":"ABCDEFAB-1234-4234-8234-ABCDEFABCDEF"})).unwrap();
        Self {
            _root: root,
            backend,
            run,
            output,
            registered,
            tool,
        }
    }
    fn session(&mut self, descriptor: &Value) {
        let script = command::source(&self.backend, "session").unwrap();
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "// session 历史来源\n").unwrap();
        let source = common::binding(&script).unwrap();
        let mut sources = self.registered["attempt"]["sources"].clone();
        sources["snapshot"]["files"] = json!([{"path":"xtask/src/recovery/clone/native/session.rs","sha256":source["sha256"]}]);
        let (number, _) = state::begin(&self.run, "post-copy", "schedules", &sources).unwrap();
        state::finish(&self.run, number, None, None).unwrap();
        let saved = state::load(&self.run, false).unwrap();
        let attempt = state::attempts(&saved).unwrap().last().unwrap().clone();
        let mut controller = json_io::read(&self.run.join("controller-0001.json")).unwrap();
        let mut original = attempt.clone();
        original["status"] = json!("running");
        for key in ["finished_at", "result", "error_type"] {
            original[key] = Value::Null;
        }
        controller["attempt"] = json!(number);
        controller["attempt_sha256"] = json!(digest::plan_hash(&original).unwrap());
        let path = self.run.join(format!("controller-{number:04}.json"));
        fs::write(&path, serde_json::to_vec(&controller).unwrap()).unwrap();
        self.registered = json!({"attempt":attempt,"controller":common::binding(&path).unwrap()});
        self.output = self
            .run
            .join("post-copy")
            .join(format!("attempt-{number:04}"));
        fs::create_dir_all(&self.output).unwrap();
        let request = registration::resolve(
            &self.backend,
            &self.run,
            "post-copy",
            descriptor,
            None,
            true,
        )
        .unwrap();
        let plan = command::build(&json!({"backend":self.backend,"run_dir":self.run,"attempt":number,"kind":"session","output":self.output,"tool":self.tool,"registration":request,"configuration":null})).unwrap();
        let launch = json!({"format_version":1,"kind":"devex-post-producer-launch","run_manifest":common::binding(&self.run.join("manifest.json")).unwrap(),
            "registration":descriptor,"controller":self.registered["controller"],"attempt":number,"producer_kind":"session","command":plan["argv"],"tool":self.tool,"source":source});
        self.rewrite("session-launch.json", &launch);
        self.rewrite("session-process.json",&json!({"format_version":1,"kind":"devex-post-producer","launch":common::binding(&self.output.join("session-launch.json")).unwrap(),
            "identity":controller["owner"]["identity"],"producer_owner":"abcdefab-1234-4234-8234-abcdefabcdef"}));
    }
    fn collect(&self) -> Result<Option<Value>> {
        super::collect(&self.backend, &self.run, &self.registered, &self.tool)
    }
    fn rewrite(&self, name: &str, value: &Value) {
        fs::write(self.output.join(name), serde_json::to_vec(value).unwrap()).unwrap();
    }
    fn relaunch(&self, launch: &Value) {
        self.rewrite("session-launch.json", launch);
        let path = self.output.join("session-process.json");
        let mut process = json_io::read(&path).unwrap();
        process["launch"] = common::binding(&self.output.join("session-launch.json")).unwrap();
        self.rewrite("session-process.json", &process);
    }
}
#[test]
fn complete_native_history_preserves_record_and_snapshot_entries() {
    let fixture = Fixture::new();
    let record = fixture.collect().unwrap().unwrap();
    assert_eq!(record["attempt"], 1);
    assert_eq!(record["stage"], "seed-runtime");
    assert_eq!(record["mode"], "identities-apply");
    assert_eq!(record["producer_kind"], "identity-apply");
    assert_eq!(record["identity"]["started"], "123");
    assert_eq!(
        record["registration"],
        common::binding(&fixture.run.join("seed-runtime.json")).unwrap()
    );
    assert_eq!(record["controller"], fixture.registered["controller"]);
    // 快照已记录来源摘要时，历史正文可以不同于当前工作文件。
    fs::write(
        command::source(&fixture.backend, "identity-apply").unwrap(),
        "// 当前版本\n",
    )
    .unwrap();
    assert_eq!(fixture.collect().unwrap().unwrap(), record);
    let mut attempt = fixture.registered["attempt"].clone();
    let script = command::source(&fixture.backend, "identity-apply").unwrap();
    let launch = json_io::read(&fixture.output.join("session-launch.json")).unwrap();
    attempt["sources"]["snapshot"]["files"][1]["sha256"] = json!("not-a-sha256");
    assert!(source::snapshot(&attempt, &fixture.backend, &script, &launch["source"]).is_err());
}
#[test]
fn missing_half_evidence_and_orphan_logs_are_rejected() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.output.join("session-process.json")).unwrap();
    assert!(fixture.collect().is_err());
    fs::remove_file(fixture.output.join("session-launch.json")).unwrap();
    assert_eq!(fixture.collect().unwrap(), None);
    fs::write(fixture.output.join("session.stderr.log"), b"").unwrap();
    assert!(fixture.collect().is_err());
}
#[test]
fn controller_owner_source_and_argv_mismatches_are_rejected() {
    for field in [
        "controller",
        "running-digest",
        "owner",
        "source",
        "argv",
        "phase",
    ] {
        let mut fixture = Fixture::new();
        match field {
            "controller" => fixture.registered["controller"]["sha256"] = json!("0".repeat(64)),
            "running-digest" => {
                let path = fixture.run.join("controller-0001.json");
                let mut controller = json_io::read(&path).unwrap();
                controller["attempt_sha256"] =
                    json!(digest::plan_hash(&fixture.registered["attempt"]).unwrap());
                fs::write(&path, serde_json::to_vec(&controller).unwrap()).unwrap();
                fixture.registered["controller"] = common::binding(&path).unwrap();
                let mut launch =
                    json_io::read(&fixture.output.join("session-launch.json")).unwrap();
                launch["controller"] = fixture.registered["controller"].clone();
                fixture.relaunch(&launch);
            }
            "owner" => {
                let mut process =
                    json_io::read(&fixture.output.join("session-process.json")).unwrap();
                process["identity_lock_owner"] = json!("abcdefab-1234-1234-8234-abcdefabcdef");
                fixture.rewrite("session-process.json", &process);
            }
            _ => {
                let mut launch =
                    json_io::read(&fixture.output.join("session-launch.json")).unwrap();
                if field == "source" {
                    launch["source"]["sha256"] = json!("0".repeat(64));
                } else if field == "phase" {
                    launch["producer_kind"] = json!("identity-verify");
                } else {
                    launch["command"][1] = json!("__wrong-producer");
                }
                fixture.relaunch(&launch);
            }
        }
        assert!(fixture.collect().is_err(), "{field}");
    }
}
#[test]
fn historical_amendment_uses_the_requested_descriptor() {
    let mut fixture = Fixture::new();
    state::initialize(&fixture.run).unwrap();
    let original = json!({"api_environment":{"original":true},"private_metadata":"unchanged"});
    json_io::write_new(&fixture.run.join("post-copy.json"), &original).unwrap();
    let base = common::binding(&fixture.run.join("post-copy.json")).unwrap();
    let (number, _) = state::begin(&fixture.run, "post-copy", "register", &json!({})).unwrap();
    state::finish(
        &fixture.run,
        number,
        Some(&json!({"status":"post_copy_registered","registration":base})),
        None,
    )
    .unwrap();
    fs::create_dir_all(fixture.run.join("post-copy/amendments")).unwrap();
    let corrected = json!({"api_environment":{"corrected":true},"private_metadata":"unchanged"});
    let path = fixture.run.join("post-copy/amendments/0001.json");
    json_io::write_new(&path, &json!({"format_version":1,"kind":"devex-clone-post-copy-amendment","sequence":1,
        "run_manifest":state::manifest_binding(&fixture.run).unwrap(),"predecessor":base,
        "request_sha256":digest::plan_hash(&corrected).unwrap(),"api_environment":corrected["api_environment"],"invalidated_runtime":null})).unwrap();
    let descriptor = common::binding(&path).unwrap();
    let (number, _) = state::begin(&fixture.run, "post-copy", "amend", &json!({})).unwrap();
    state::finish(&fixture.run,number,Some(&json!({"status":"post_copy_amended","registration":descriptor,"sequence":1,"predecessor":base,
        "runtime_directory":fixture.run.join("post-copy").join("runtime-0001"),"remote_writes":0,"worker_must_remain_stopped":true,"restore_qualified":false})),None).unwrap();
    assert_eq!(
        registration::resolve(
            &fixture.backend,
            &fixture.run,
            "post-copy",
            &descriptor,
            None,
            true
        )
        .unwrap(),
        corrected
    );
    assert_eq!(
        registration::resolve(
            &fixture.backend,
            &fixture.run,
            "post-copy",
            &base,
            None,
            true
        )
        .unwrap(),
        original
    );
    fixture.session(&base);
    fixture.tool =
        json!({"path":fixture.backend.join("different-coordinator.exe"),"sha256":"0".repeat(64)});
    let record = fixture.collect().unwrap().unwrap();
    assert_eq!(record["registration"], base);
    let mut launch = json_io::read(&fixture.output.join("session-launch.json")).unwrap();
    launch["registration"] = descriptor.clone();
    fixture.relaunch(&launch);
    assert_eq!(
        fixture.collect().unwrap().unwrap()["registration"],
        descriptor
    );
}
