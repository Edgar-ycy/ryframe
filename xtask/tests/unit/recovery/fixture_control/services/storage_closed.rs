use super::*;
use crate::recovery::clone::native::lifecycle::{lock::RunLock, state};
use std::{net::TcpListener, path::PathBuf};

struct Fixture {
    _temporary: tempfile::TempDir,
    backend: PathBuf,
    run: PathBuf,
    attempt: Value,
    stopped: PathBuf,
}

fn listener() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    (listener, url)
}

fn credential(run: &Path, name: &str, value: &str) -> Value {
    #[cfg(windows)]
    let path = PathBuf::from(run.to_str().unwrap().replace('\\', "/")).join(name);
    #[cfg(not(windows))]
    let path = run.join(name);
    document::write_new(&path, &json!(value)).unwrap();
    crate::recovery::fixture_control::file_binding(&path, "单位凭据").unwrap()
}

#[test]
fn early_reader_accepts_original_credential_paths_but_rejects_snapshot_tampering() {
    let fixture = Fixture::new();
    read(&fixture.backend, &fixture.run, &fixture.attempt).unwrap();
    let actor_path = fixture.run.join("rustfs/actor.json");
    let descriptor_path = fixture.run.join("rustfs/actor-binding.json");
    let original_actor = std::fs::read(&actor_path).unwrap();
    let original_descriptor = std::fs::read(&descriptor_path).unwrap();
    let actor = document::read(&actor_path).unwrap();
    let index = |key: &str| {
        let path = Path::new(
            actor["request"]["credential_files"][key]["path"]
                .as_str()
                .unwrap(),
        );
        actor["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .position(|snapshot| Path::new(snapshot["binding"]["path"].as_str().unwrap()) == path)
            .unwrap()
    };
    let (access, secret) = (index("access_key"), index("secret_key"));
    for change in ["duplicate", "bytes", "sha256", "inode"] {
        let mut changed = actor.clone();
        match change {
            "duplicate" => {
                let duplicate = changed["snapshots"][access].clone();
                changed["snapshots"][secret] = duplicate;
            }
            "bytes" => {
                changed["snapshots"][access]["binding"]["bytes"] = json!(
                    actor["snapshots"][access]["binding"]["bytes"]
                        .as_u64()
                        .unwrap()
                        + 1
                );
            }
            "sha256" => changed["snapshots"][access]["binding"]["sha256"] = json!("0".repeat(64)),
            "inode" => {
                changed["snapshots"][access]["inode"] =
                    json!(actor["snapshots"][access]["inode"].as_u64().unwrap() ^ 1);
            }
            _ => unreachable!(),
        }
        // 仅单位夹具重签 actor 描述符，以实际检验下游快照拒绝；不构成服务关闭证明。
        std::fs::write(&actor_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        std::fs::write(
            &descriptor_path,
            serde_json::to_vec(&binding::snapshot(&actor_path).unwrap()).unwrap(),
        )
        .unwrap();
        assert!(
            read(&fixture.backend, &fixture.run, &fixture.attempt).is_err(),
            "{change}"
        );
    }
    std::fs::write(&actor_path, original_actor).unwrap();
    std::fs::write(&descriptor_path, original_descriptor).unwrap();
    read(&fixture.backend, &fixture.run, &fixture.attempt).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let temporary = tempfile::tempdir_in(backend.join(".local-tests")).unwrap();
        let run = temporary.path().to_path_buf();
        let output = run.join("rustfs");
        let data = run.join("data");
        std::fs::create_dir(&output).unwrap();
        std::fs::create_dir(&data).unwrap();
        let put = |name: &str, value: &Value| {
            let path = run.join(name);
            document::write_new(&path, value).unwrap();
            common::binding(&path).unwrap()
        };
        let external = run.join("rustfs-fixture.bin");
        std::fs::write(&external, b"unit-only-original-tool").unwrap();
        let executable = json!({"path":external,"sha256":digest::file(&external).unwrap().sha256});
        let credentials = json!({"access_key":credential(&run,"access.txt","unit-access"),"secret_key":credential(&run,"secret.txt","unit-secret")});
        let private = json!({"APP_ENV":"test"});
        let (api_listener, api) = listener();
        let (console_listener, console) = listener();
        let environment = put("environment.json", &json!({"environment":private}));
        let bootstrap = put(
            "bootstrap.json",
            &json!({"environment_sha256":digest::plan_hash(&private).unwrap(),
            "secret_files":{"rustfs-access-key.txt":credentials["access_key"],"rustfs-secret-key.txt":credentials["secret_key"]}}),
        );
        let review = put(
            "review.json",
            &json!({"tools":{"rustfs":executable},"services":{"rustfs":{
            "scope_id":"closed-risk","api":api,"console":console,"data_dir":data}}}),
        );
        let manifest = put(
            "manifest.json",
            &json!({"format_version":1,"kind":"reference-fixture-service-run",
            "review":review,"bootstrap":bootstrap,"execution_backend":backend,"scope_id":"closed-risk","data_directory_was_empty":true}),
        );
        state::initialize(&run).unwrap();
        let (device, inode) =
            common::device_inode_for(&data, &std::fs::metadata(&data).unwrap()).unwrap();
        let request = json!({"scope_id":"closed-risk","executable":executable,"data_directory":{"path":data,"device":device,"inode":inode},
            "api_url":api,"console_url":console,"credential_files":credentials,"timeout_seconds":60});
        document::write_new(&output.join("request.json"), &request).unwrap();
        let request_binding = common::binding(&output.join("request.json")).unwrap();
        let lock = RunLock::claim(&run).unwrap();
        let (number, attempt) = state::begin(
            &run,
            "storage-target",
            "initial",
            &json!({"review":review,"bootstrap":bootstrap,"request":request_binding}),
        )
        .unwrap();
        state::bind_controller(&run, number, &lock.owner, &attempt).unwrap();
        let controller = common::binding(&state::controller_path(&run, number)).unwrap();
        let owner = common::binding(&run.join("run.lock/owner.json")).unwrap();
        let expected = [
            request_binding,
            manifest.clone(),
            controller.clone(),
            review,
            owner.clone(),
            bootstrap,
            environment,
            credentials["access_key"].clone(),
            credentials["secret_key"].clone(),
            common::binding(&external).unwrap(),
        ];
        let snapshots = expected
            .iter()
            .map(|v| binding::snapshot(Path::new(v["path"].as_str().unwrap())).unwrap())
            .collect();
        let actor = actor::Actor {
            backend: backend.clone(),
            output,
            request,
            private,
            manifest,
            controller,
            number,
            operation_id: "a".repeat(32),
            tool: common::binding(&std::env::current_exe().unwrap()).unwrap(),
            snapshots,
            owner_binding: owner,
        };
        let (attempt, stopped) = Self::close_actor(&actor, &run, &lock, number);
        drop((api_listener, console_listener));
        Self {
            _temporary: temporary,
            backend,
            run,
            attempt,
            stopped,
        }
    }
    fn close_actor(
        actor: &actor::Actor,
        run: &Path,
        lock: &RunLock,
        number: u64,
    ) -> (Value, PathBuf) {
        document::write_new(&actor.path("intent.json"),&json!({"format_version":1,"kind":"devex-clone-storage-intent",
            "request":actor.manifest,"controller":actor.controller,"attempt":number,"arguments":actor.arguments().unwrap()})).unwrap();
        document::write_new(
            &actor.path("actor.json"),
            &serde_json::to_value(actor).unwrap(),
        )
        .unwrap();
        document::write_new(
            &actor.path("actor-binding.json"),
            &binding::snapshot(&actor.path("actor.json")).unwrap(),
        )
        .unwrap();
        let identity = |pid| json!({"pid":pid,"started":"1","executable":actor.tool["path"]});
        let ready = json!({"format_version":1,"operation_id":actor.operation_id,"directory":actor.output,"role":"rustfs",
            "scope_id":"closed-risk","supervisor":identity(4_000_000_001_u32),"monitor":identity(4_000_000_002_u32)});
        document::write_new(&actor.member_path("ready"), &ready).unwrap();
        let mut proof = ready;
        proof["status"] = json!("stopped");
        proof["error_type"] = Value::Null;
        proof["members"] = json!([proof["supervisor"]]);
        let stopped = actor.member_path("stopped");
        document::write_new(&stopped, &proof).unwrap();
        state::finish(run, number, None, Some("UnitEarlyFailure")).unwrap();
        lock.release().unwrap();
        let attempt = state::load(run, true).unwrap()["attempts"][0].clone();
        (attempt, stopped)
    }
}

#[test]
fn early_reader_rejects_foreign_failed_or_missing_member_proof_without_creating_replacements() {
    let fixture = Fixture::new();
    // 只测读侧拒绝合同；手工单位夹具不是实际 Job/pidfd 关闭验收证据。
    let original = document::read(&fixture.stopped).unwrap();
    std::fs::remove_file(&fixture.stopped).unwrap();
    super::super::records::load(&fixture.backend, &fixture.run, &fixture.attempt).unwrap();
    document::write_new(&fixture.stopped, &original).unwrap();
    let actor_path = fixture.run.join("rustfs/actor.json");
    let descriptor_path = fixture.run.join("rustfs/actor-binding.json");
    let original_actor = std::fs::read(&actor_path).unwrap();
    let original_descriptor = std::fs::read(&descriptor_path).unwrap();
    let actor = document::read(&actor_path).unwrap();
    let foreign = fixture.run.join("foreign-tool.bin");
    std::fs::write(&foreign, b"foreign-tool").unwrap();
    let mut wrong_digest = actor["tool"].clone();
    wrong_digest["sha256"] = json!("0".repeat(64));
    for tool in [common::binding(&foreign).unwrap(), wrong_digest] {
        let mut changed = actor.clone();
        changed["tool"] = tool;
        std::fs::write(&actor_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        std::fs::write(
            &descriptor_path,
            serde_json::to_vec(&binding::snapshot(&actor_path).unwrap()).unwrap(),
        )
        .unwrap();
        assert!(
            super::super::records::load(&fixture.backend, &fixture.run, &fixture.attempt).is_err()
        );
    }
    std::fs::write(&actor_path, original_actor).unwrap();
    std::fs::write(&descriptor_path, original_descriptor).unwrap();
    super::super::records::load(&fixture.backend, &fixture.run, &fixture.attempt).unwrap();
    for (field, replacement) in [
        ("status", json!("failed")),
        ("members", json!([])),
        (
            "monitor",
            json!({"pid":4_000_000_003_u32,"started":"1","executable":std::env::current_exe().unwrap()}),
        ),
    ] {
        let mut changed = original.clone();
        changed[field] = replacement;
        document::write_replace(&fixture.stopped, &changed).unwrap();
        assert!(read(&fixture.backend, &fixture.run, &fixture.attempt).is_err());
    }
    std::fs::remove_file(&fixture.stopped).unwrap();
    assert!(read(&fixture.backend, &fixture.run, &fixture.attempt).is_err());
    let output = fixture.run.join("rustfs");
    assert!(!output.join("rustfs-tree.json").exists());
    assert!(!output.join("not-started.json").exists());
    assert!(!output.join("ready.json").exists());
    assert!(!output.read_dir().unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .contains("-control.json")
    }));
}
