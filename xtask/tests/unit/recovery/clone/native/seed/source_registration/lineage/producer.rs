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
        let parent = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let root = tempfile::Builder::new()
            .prefix("producer-lineage-")
            .tempdir_in(parent)
            .unwrap();
        let backend = portable_path(&fs::canonicalize(root.path()).unwrap());
        let run = backend.join(".local-tests/run");
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join("manifest.json"), b"{}").unwrap();
        state::initialize(&run).unwrap();
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
        let (number, running) =
            state::begin(&run, "seed-runtime", "identities-apply", &snapshot).unwrap();
        let output = run
            .join("seed-runtime")
            .join(format!("attempt-{number:04}"));
        fs::create_dir_all(&output).unwrap();
        let identity = json!({"pid":42,"started":"123","executable":executable});
        let controller = json!({"format_version":1,"kind":"devex-stage-controller","attempt":number,
            "attempt_sha256":digest::plan_hash(&running).unwrap(),"owner":{"format_version":1,"identity":identity,"directory":run,"manifest_sha256":common::binding(&run.join("manifest.json")).unwrap()["sha256"]}});
        let controller_path = state::controller_path(&run, number);
        json_io::write_new(&controller_path, &controller).unwrap();
        let controller_binding = common::binding(&controller_path).unwrap();
        let plan = command::build(&json!({"backend":backend,"run_dir":run,"attempt":number,"kind":"identity-apply","output":output,"tool":tool,"registration":request,"configuration":null})).unwrap();
        let launch = json!({"format_version":1,"kind":"devex-post-producer-launch","run_manifest":common::binding(&run.join("manifest.json")).unwrap(),
            "registration":common::binding(&run.join("seed-runtime.json")).unwrap(),"controller":controller_binding,"attempt":number,"producer_kind":"identity-apply",
            "command":plan["argv"],"tool":tool,"source":descriptor});
        json_io::write_new(&output.join("session-launch.json"), &launch).unwrap();
        json_io::write_new(&output.join("session-process.json"), &json!({"format_version":1,"kind":"devex-post-producer",
            "launch":common::binding(&output.join("session-launch.json")).unwrap(),"identity":identity,"identity_lock_owner":"ABCDEFAB-1234-4234-8234-ABCDEFABCDEF"})).unwrap();
        state::finish(
            &run,
            number,
            Some(&json!({"status":"identity_producer_finished"})),
            None,
        )
        .unwrap();
        let saved = state::load(&run, true).unwrap();
        let attempt = state::attempts(&saved).unwrap().last().unwrap().clone();
        let registered =
            json!({"attempt":attempt,"controller":common::binding(&controller_path).unwrap()});
        Self {
            _root: root,
            backend,
            run,
            output,
            registered,
            tool,
        }
    }
    fn post_copy(&mut self, descriptor: &Value, kind: &str) {
        let script = command::source(&self.backend, kind).unwrap();
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "// post-copy 历史来源\n").unwrap();
        let source = common::binding(&script).unwrap();
        let relative = script
            .strip_prefix(&self.backend)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/");
        let mut sources = self.registered["attempt"]["sources"].clone();
        sources["snapshot"]["files"] = json!([{"path":relative,"sha256":source["sha256"]}]);
        let mode = if kind == "existing" {
            "verify"
        } else {
            "schedules"
        };
        let (number, running) = state::begin(&self.run, "post-copy", mode, &sources).unwrap();
        let previous = PathBuf::from(self.registered["controller"]["path"].as_str().unwrap());
        let mut controller = json_io::read(&previous).unwrap();
        controller["attempt"] = json!(number);
        controller["attempt_sha256"] = json!(digest::plan_hash(&running).unwrap());
        let path = state::controller_path(&self.run, number);
        json_io::write_new(&path, &controller).unwrap();
        let controller_binding = common::binding(&path).unwrap();
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
        let configuration = (kind == "existing").then(|| {
            let path = self.output.join("existing-configuration.json");
            json_io::write_new(&path, &json!({"fixture":1})).unwrap();
            path
        });
        let plan = command::build(&json!({"backend":self.backend,"run_dir":self.run,"attempt":number,"kind":kind,"output":self.output,"tool":self.tool,"registration":request,"configuration":configuration})).unwrap();
        let mut launch = json!({"format_version":1,"kind":"devex-post-producer-launch","run_manifest":common::binding(&self.run.join("manifest.json")).unwrap(),
            "registration":descriptor,"controller":controller_binding,"attempt":number,"producer_kind":kind,"command":plan["argv"],"tool":self.tool,"source":source});
        let mut process = json!({"format_version":1,"kind":"devex-post-producer",
            "identity":controller["owner"]["identity"]});
        if let Some(configuration) = configuration {
            launch["configuration"] = common::binding(&configuration).unwrap();
        } else {
            process["producer_owner"] = json!("abcdefab-1234-4234-8234-abcdefabcdef");
        }
        self.rewrite("session-launch.json", &launch);
        process["launch"] = common::binding(&self.output.join("session-launch.json")).unwrap();
        self.rewrite("session-process.json", &process);
        state::finish(
            &self.run,
            number,
            Some(&json!({"status":"historical_session_finished"})),
            None,
        )
        .unwrap();
        let saved = state::load(&self.run, true).unwrap();
        let attempt = state::attempts(&saved).unwrap().last().unwrap().clone();
        self.registered = json!({"attempt":attempt,"controller":common::binding(&path).unwrap()});
    }
    fn existing() -> Self {
        let mut fixture = Self::new();
        let path = fixture.run.join("post-copy.json");
        let request = json!({"reference_plan":{"path":fixture.run.join("reference-plan.json")},"dataset":{"path":fixture.run.join("dataset.json")}});
        json_io::write_new(&path, &request).unwrap();
        let descriptor = common::binding(&path).unwrap();
        let (number, _) = state::begin(&fixture.run, "post-copy", "register", &json!({})).unwrap();
        state::finish(
            &fixture.run,
            number,
            Some(&json!({"status":"post_copy_registered","registration":descriptor})),
            None,
        )
        .unwrap();
        fixture.post_copy(&descriptor, "existing");
        fixture
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
        "controller-extra",
        "owner",
        "missing-owner",
        "owner-key",
        "process-extra",
        "executable",
        "source",
        "argv",
        "phase",
        "launch-extra",
        "launch-controller",
        "registration",
    ] {
        let mut fixture = Fixture::new();
        match field {
            "controller" => fixture.registered["controller"]["sha256"] = json!("0".repeat(64)),
            "running-digest" | "controller-extra" => {
                let path = fixture.run.join("controller-0001.json");
                let mut controller = json_io::read(&path).unwrap();
                if field == "running-digest" {
                    controller["attempt_sha256"] =
                        json!(digest::plan_hash(&fixture.registered["attempt"]).unwrap());
                } else {
                    controller["unexpected"] = json!(true);
                }
                fs::write(&path, serde_json::to_vec(&controller).unwrap()).unwrap();
                fixture.registered["controller"] = common::binding(&path).unwrap();
                let mut launch =
                    json_io::read(&fixture.output.join("session-launch.json")).unwrap();
                launch["controller"] = fixture.registered["controller"].clone();
                fixture.relaunch(&launch);
            }
            "owner" | "missing-owner" | "owner-key" | "process-extra" | "executable" => {
                let mut process =
                    json_io::read(&fixture.output.join("session-process.json")).unwrap();
                match field {
                    "owner" => {
                        process["identity_lock_owner"] =
                            json!("abcdefab-1234-1234-8234-abcdefabcdef");
                    }
                    "missing-owner" | "owner-key" => {
                        let owner = process
                            .as_object_mut()
                            .unwrap()
                            .remove("identity_lock_owner")
                            .unwrap();
                        if field == "owner-key" {
                            process["producer_owner"] = owner;
                        }
                    }
                    "process-extra" => process["unexpected"] = json!(true),
                    _ => {
                        let other = fixture.backend.join("other.exe");
                        fs::write(&other, b"other historical tool").unwrap();
                        process["identity"]["executable"] = json!(other);
                    }
                }
                fixture.rewrite("session-process.json", &process);
            }
            _ => {
                let mut launch =
                    json_io::read(&fixture.output.join("session-launch.json")).unwrap();
                if field == "source" {
                    launch["source"]["sha256"] = json!("0".repeat(64));
                } else if field == "phase" {
                    launch["producer_kind"] = json!("identity-verify");
                } else if field == "launch-extra" {
                    launch["unexpected"] = json!(true);
                } else if field == "launch-controller" {
                    launch["controller"] =
                        common::binding(&fixture.run.join("manifest.json")).unwrap();
                } else if field == "registration" {
                    launch["registration"] =
                        common::binding(&fixture.run.join("manifest.json")).unwrap();
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
    fixture.post_copy(&base, "session");
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

#[test]
fn native_existing_history_uses_its_controller_and_is_independent_of_current_tool() {
    let mut fixture = Fixture::existing();
    let original = fixture.tool.clone();
    let current = fixture.backend.join("current-coordinator.exe");
    fs::write(&current, b"current coordinator").unwrap();
    fixture.tool = json!({"path":current,"sha256":digest::file(&current).unwrap().sha256});

    let record = fixture.collect().unwrap().unwrap();
    assert_eq!(record["producer_kind"], "existing");
    assert_eq!(record["mode"], "verify");
    assert_eq!(record["identity"]["executable"], original["path"]);
    assert_eq!(record["controller"], fixture.registered["controller"]);
}

#[test]
fn native_existing_history_rejects_configuration_and_controller_drift() {
    for change in [
        "configuration-path",
        "configuration-content",
        "controller-tool",
    ] {
        let mut fixture = Fixture::existing();
        let mut launch = json_io::read(&fixture.output.join("session-launch.json")).unwrap();
        let configuration = fixture.output.join("existing-configuration.json");
        match change {
            "configuration-path" => {
                let replacement = fixture.output.join("replacement-configuration.json");
                fs::copy(&configuration, &replacement).unwrap();
                launch["configuration"] = common::binding(&replacement).unwrap();
                let index = launch["command"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|value| value == "--configuration")
                    .unwrap();
                launch["command"][index + 1] = json!(replacement);
                fixture.relaunch(&launch);
            }
            "configuration-content" => {
                fixture.rewrite("existing-configuration.json", &json!({"fixture":2}));
            }
            _ => {
                let other = fixture.backend.join("other-controller.exe");
                fs::copy(fixture.backend.join("tool.exe"), &other).unwrap();
                let path = source::binding_path(&fixture.registered["controller"]).unwrap();
                let mut controller = json_io::read(&path).unwrap();
                controller["owner"]["identity"]["executable"] = json!(other);
                fs::write(&path, serde_json::to_vec(&controller).unwrap()).unwrap();
                fixture.registered["controller"] = common::binding(&path).unwrap();
                launch["controller"] = fixture.registered["controller"].clone();
                fixture.relaunch(&launch);
            }
        }
        assert!(fixture.collect().is_err(), "{change}");
    }
}
