//! 登记首代、正常关闭和完整restart结果；只准备本地历史，不启动服务。
use crate::recovery::{
    clone::native::{
        cache::model::{cygwin_path, render_configuration},
        digest, json as document,
        lifecycle::{lock::RunLock, state},
        resources::common,
    },
    fixture_control::services::history,
};
use crate::target_verify_fixture::{Fixture, SCOPE, tool};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(crate) struct Restart {
    pub(crate) storage: Value,
    pub(crate) cache: Value,
}

const REDIS_PORT: u16 = 26379;
const REDIS_PASSWORD: &str = "fixture-redis";

pub(crate) fn windows_redis_files(fixture: &Fixture, redis: &Path) -> Value {
    let package = fixture.local.join("redis-package");
    fs::create_dir(&package).unwrap();
    let program = package.join("redis-server.exe");
    let dll = package.join("msys-2.0.dll");
    fs::write(&program, b"fixture Windows Redis executable").unwrap();
    fs::write(&dll, b"fixture Windows Redis dependency").unwrap();
    let configuration = redis.join("redis.conf");
    fs::write(
        &configuration,
        render_configuration(".", REDIS_PORT, REDIS_PASSWORD),
    )
    .unwrap();
    let package = json!({"platform":"windows","path":program,"sha256":tool(&program)["sha256"],
        "version":"Redis server v=7.4.11 bits=64","dlls":{"msys-2.0.dll":tool(&dll)["sha256"]}});
    json!({"platform":"windows","port":REDIS_PORT,"pid":200,"started":"67890","executable":program,
        "sha256":package["sha256"],"run_id":"3".repeat(40),
        "configuration":common::binding(&configuration).unwrap(),"tool":package})
}

pub(crate) fn register(f: &Fixture) -> Restart {
    let run = f.local.join("registered-restart");
    initial(f, &run);
    restart(f, &run)
}

pub(crate) fn initial(f: &Fixture, run: &Path) {
    fs::create_dir(run).unwrap();
    let mut review = f.request["review"].clone();
    review.as_object_mut().unwrap().remove("canonical_sha256");
    save(
        &run.join("bootstrap.json"),
        &json!({"fixture":"isolated-services"}),
    );
    save(
        &run.join("manifest.json"),
        &json!({"format_version":1,"kind":"reference-fixture-service-run",
        "review":review,"bootstrap":common::binding(&run.join("bootstrap.json")).unwrap(),
        "execution_backend":f.backend,"scope_id":SCOPE,"data_directory_was_empty":true}),
    );
    state::initialize(run).unwrap();
    let redis = &f.request["storage"]["redis"];
    let output = run.join("redis");
    let redis_result = if redis["platform"] == "windows" {
        windows_initial(f, &output, redis)
    } else {
        save(
            &output.join("request.json"),
            &json!({"scope_id":SCOPE,"configuration":redis["configuration"],
                "previous_identity":null,"previous_boot_id":null,"previous_run_id":null}),
        );
        json!({"runtime":{"redis":redis}})
    };
    for (stage, mode, mut value) in [
        (
            "storage-target",
            "initial",
            f.request["storage"]["rustfs"].clone(),
        ),
        ("cache-target", "initial", redis_result),
        (
            "fixture-buckets",
            "prepare",
            json!({"status":"buckets_ready"}),
        ),
    ] {
        let windows_cache = stage == "cache-target" && redis["platform"] == "windows";
        let sources = if windows_cache {
            json!({"manifest":common::binding(&run.join("manifest.json")).unwrap(),
                "request":common::binding(&output.join("request.json")).unwrap()})
        } else {
            json!({})
        };
        let owner = windows_cache.then(|| RunLock::claim(run).unwrap());
        let (number, attempt) = state::begin(run, stage, mode, &sources).unwrap();
        if let Some(owner) = &owner {
            state::bind_controller(run, number, &owner.owner, &attempt).unwrap();
            value["controller"] = common::binding(&state::controller_path(run, number)).unwrap();
        }
        state::finish(run, number, Some(&value), None).unwrap();
        if let Some(owner) = owner {
            owner.release().unwrap();
        }
    }
    let storage_request = json!({"data_directory":directory(f),
        "api_url":"http://127.0.0.1:29000","console_url":"http://127.0.0.1:29001"});
    save(&run.join("rustfs/request.json"), &storage_request);
}

pub(crate) fn restart(f: &Fixture, run: &Path) -> Restart {
    let storage_request =
        crate::recovery::clone::native::json::read(&run.join("rustfs/request.json")).unwrap();
    let redis_request =
        crate::recovery::clone::native::json::read(&run.join("redis/request.json")).unwrap();
    let review =
        crate::recovery::clone::native::json::read(&run.join("manifest.json")).unwrap()["review"]
            .clone();
    let previous = close(run);
    let sources = json!({"manifest":common::binding(&run.join("manifest.json")).unwrap(),
        "state_before":common::binding(&run.join("state.json")).unwrap(),"restart_predecessor":previous,"review":review});
    let owner = RunLock::claim(run).unwrap();
    let (number, attempt) = state::begin(run, "fixture-services", "restart", &sources).unwrap();
    state::bind_controller(run, number, &owner.owner, &attempt).unwrap();
    let controller = common::binding(&state::controller_path(run, number)).unwrap();
    let root = run.join(format!("lifecycle-{number:04}"));
    let (storage, redis) = running(f, run, &root, &storage_request, &redis_request);
    let proof = json!({"format_version":1,"kind":"reference-fixture-service-generation","run":run,
        "attempt":number,"owner":sources["state_before"],"predecessor":previous,
        "previous_generation":{"kind":"initial","rustfs":common::binding(&run.join("results/0001.json")).unwrap(),
            "redis":common::binding(&run.join("results/0002.json")).unwrap()},
        "controller":controller,"services":{"redis":"running","rustfs":"running"},
        "rustfs":storage,"redis":redis,"remote_writes":0,"resources_deleted":false});
    let generation = save(&root.join("restart.json"), &proof);
    let result = json!({"kind":"reference-fixture-service-lifecycle","operation":"restart","run":run,
        "status":"services_restarted","services":{"redis":"running","rustfs":"running"},
        "generation":generation,"controller":controller,"remote_writes":0,"resources_deleted":false});
    state::finish(run, number, Some(&result), None).unwrap();
    owner.release().unwrap();
    let actual = history::validate(run, &state::load(run, true).unwrap(), true, true).unwrap();
    let bound = actual["active_generation"].clone();
    assert_eq!(
        bound,
        common::binding(&state::result_path(run, number)).unwrap()
    );
    let mut projection = storage["storage"].clone();
    projection.as_object_mut().unwrap().remove("tree");
    Restart {
        storage: json!({"storage":projection,"data_directory":storage_request["data_directory"],
            "api_url":storage_request["api_url"],"console_url":storage_request["console_url"],"generation":bound}),
        cache: json!({"redis":redis["runtime"]["redis"],"generation":bound}),
    }
}

#[test]
fn fixture_arm_storage_keeps_frozen_history_and_requires_the_exact_active_generation() {
    use crate::recovery::clone::native::lifecycle::manifest::{self, fixture_storage};

    let f = Fixture::new();
    let run = f.local.join("arm-service-run");
    initial(&f, &run);
    let historical = || {
        json!({"path":run,"manifest":common::binding(&run.join("manifest.json")).unwrap(),
        "state":common::binding(&run.join("state.json")).unwrap()})
    };
    let register = |name: &str| {
        let workspace = f.local.join(name);
        fs::create_dir_all(workspace.join("target")).unwrap();
        save(
            &workspace.join("registration.json"),
            &json!({
                "format_version":1, "kind":"devex-clone-fresh-target-registration",
                "request":common::binding(&f.target.join("request.json")).unwrap(),
                "environment":{}, "storage_run":historical(), "target_directory":workspace.join("target"),
            }),
        )
    };
    let registration = register("arm-initial");
    let resolve =
        |binding, active| fixture_storage::resolve(&f.backend, &registration, binding, active);
    let initial = resolve(None, false).unwrap().unwrap();
    assert_eq!(initial.binding["generation"]["kind"], "initial");
    assert_eq!(
        initial.binding["generation"]["rustfs"],
        common::binding(&run.join("results/0001.json")).unwrap()
    );
    assert_eq!(
        initial.binding["generation"]["redis"],
        common::binding(&run.join("results/0002.json")).unwrap()
    );
    manifest::validate_target_storage(&f.backend, &registration, &initial.binding).unwrap();

    let restarted = restart(&f, &run);
    assert!(initial.check(&f.backend).is_err());
    assert!(resolve(Some(&initial.binding), false).unwrap().is_some());
    assert!(resolve(Some(&initial.binding), true).is_err());
    let active = resolve(None, false).unwrap().unwrap();
    assert_eq!(
        active.binding["generation"],
        restarted.storage["generation"]
    );
    assert!(resolve(Some(&active.binding), true).unwrap().is_some());

    let new_registration = register("arm-restarted");
    assert!(
        fixture_storage::resolve(&f.backend, &new_registration, Some(&initial.binding), false)
            .is_err()
    );
    let mut forged = active.binding.clone();
    forged["generation"]["sha256"] = json!("0".repeat(64));
    assert!(resolve(Some(&forged), false).is_err());
    let mut forged = active.binding.clone();
    forged["cache"] = json!({});
    assert!(resolve(Some(&forged), false).is_err());
    external_arm_storage_guard(&f, &registration, &active.binding);

    state::begin(&run, "fixture-services", "close", &json!({})).unwrap();
    assert!(active.check(&f.backend).is_err());
    assert!(resolve(Some(&active.binding), true).is_err());
}

#[test]
fn arm_storage_guard_also_accepts_the_original_independent_clone_history() {
    let f = Fixture::new();
    let run = f.local.join("arm-independent-storage");
    let (mut history, effective) = crate::clone_lifecycle_tests::arm_storage_bindings(&run);
    history["state"] = common::binding(&run.join("state.json")).unwrap();
    let registration = save(
        &f.local.join("arm-independent/registration.json"),
        &json!({"storage_run":history}),
    );
    external_arm_storage_guard(&f, &registration, &effective);
}

fn external_arm_storage_guard(f: &Fixture, registration: &Value, effective: &Value) {
    use crate::recovery::clone::native::{
        cache::handoff::locks::Storage,
        lifecycle::{lock::ProcessGuard, manifest},
    };

    manifest::validate_target_storage(&f.backend, registration, effective).unwrap();
    let outer_guard = ProcessGuard::acquire(&f.run).unwrap();
    let outer = RunLock::claim(&f.run).unwrap();
    let arm = json!({"copy_stage":"seed_to_arm","target_registration":registration,
        "target_storage_run":effective});
    let storage = Storage::acquire(&f.backend, &f.run, &arm).unwrap();
    let path = Path::new(effective["path"].as_str().unwrap());
    assert_eq!(storage.directory, path);
    assert!(path.join("run.lock/owner.json").is_file());
    assert!(Storage::acquire(&f.backend, &f.run, &arm).is_err());
    assert!(
        storage
            .finish::<()>(Err("目标核验测试失败".into()))
            .is_err()
    );
    assert!(!path.join("run.lock").exists());
    assert!(f.run.join("run.lock/owner.json").is_file());
    Storage::acquire(&f.backend, &f.run, &arm)
        .unwrap()
        .finish(Ok(()))
        .unwrap();
    outer.release().unwrap();
    outer_guard.check().unwrap();
}

fn directory(f: &Fixture) -> Value {
    let path = f.local.join("storage/objects");
    directory_binding(&path)
}

fn directory_binding(path: &Path) -> Value {
    let (device, inode) = common::device_inode_for(path, &fs::metadata(path).unwrap()).unwrap();
    json!({"path":path,"device":device,"inode":inode})
}

fn save(path: &Path, value: &Value) -> Value {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    crate::recovery::clone::native::json::write_new(path, value).unwrap();
    common::binding(path).unwrap()
}

pub(crate) fn close(run: &Path) -> Value {
    let sources = json!({"manifest":common::binding(&run.join("manifest.json")).unwrap(),
        "state_before":common::binding(&run.join("state.json")).unwrap()});
    let (number, _) = state::begin(run, "fixture-services", "close", &sources).unwrap();
    let root = run.join(format!("lifecycle-{number:04}"));
    let tree = json!({"format_version":2,"runtime_directory":run.join("rustfs"),"role":"rustfs",
        "scope_id":SCOPE,"operation_id":"b".repeat(32),"supervisor":{"pid":100},"monitor":{"pid":101},"process":{"pid":42}});
    let completion = save(
        &run.join(format!(
            "rustfs/rustfs-members-{}-stopped.json",
            "b".repeat(32)
        )),
        &json!({"status":"stopped","error_type":null,"operation_id":tree["operation_id"],
            "role":tree["role"],"scope_id":SCOPE,"supervisor":tree["supervisor"],"monitor":tree["monitor"],
            "directory":tree["runtime_directory"],"members":[tree["supervisor"],tree["monitor"],tree["process"]]}),
    );
    let redis_request = document::read(&run.join("redis/request.json")).unwrap();
    let redis = if redis_request["format_version"] == 3 {
        let runtime = document::read(&run.join("redis/runtime.json")).unwrap();
        let exited = save(
            &run.join("redis/exited.json"),
            &json!({"process":runtime["process_receipt"],"returncode":0,"job_empty":true}),
        );
        json!({"state":"stopped","status":"redis_process_stopped","runtime":runtime,
            "resources_deleted":false,"windows":{"identity":runtime["process"],"alive":false},
            "exited":exited})
    } else {
        json!({"status":"redis_process_stopped","alive":false,"resources_deleted":false})
    };
    let redis = save(&root.join("stopped.json"), &redis);
    let rustfs = save(
        &root.join("rustfs-stopped.json"),
        &json!({"state":"stopped","tree":tree,"completion":completion}),
    );
    let result = json!({"kind":"reference-fixture-service-lifecycle","operation":"close","run":run,
        "status":"services_closed","services":{"redis":"stopped","rustfs":"stopped"},
        "evidence":{"redis":redis,"rustfs":rustfs},"remote_writes":0,"resources_deleted":false});
    state::finish(run, number, Some(&result), None).unwrap();
    history::validate(run, &state::load(run, true).unwrap(), true, true).unwrap();
    common::binding(&state::result_path(run, number)).unwrap()
}

fn windows_initial(f: &Fixture, output: &Path, redis: &Value) -> Value {
    let configuration = Path::new(redis["configuration"]["path"].as_str().unwrap());
    let request = json!({"format_version":3,"scope_id":format!("services-{SCOPE}"),"tool":redis["tool"],
        "configuration":redis["configuration"],"directory":directory_binding(configuration.parent().unwrap()),
        "previous_identity":null,"previous_run_id":null,"port":REDIS_PORT,"password":REDIS_PASSWORD,
        "timeout_seconds":60});
    fs::write(
        f.local.join("redis-coordinator.exe"),
        b"fixture coordinator",
    )
    .unwrap();
    save(&output.join("request.json"), &request);
    let runtime = windows_runtime(f, output, &request, redis);
    save(&output.join("runtime.json"), &runtime);
    json!({"service":"redis","runtime":runtime,"request_sha256":digest::plan_hash(&request).unwrap(),
        "remote_writes":0})
}

/// 仅登记本地历史读取所需的 v3 证明；PID 是测试数据，未启动进程或连接端口。
fn windows_runtime(f: &Fixture, output: &Path, request: &Value, redis: &Value) -> Value {
    let executable = f.local.join("redis-coordinator.exe");
    let owner = json!({"pid":202,"started":"67900","executable":executable});
    let supervisor = json!({"pid":redis["pid"].as_u64().unwrap()+10,
        "started":redis["started"],"executable":executable});
    let launch = save(
        &output.join("launch-request.json"),
        &json!({
            "format_version":3,"backend":f.backend,"output":output,"owner":owner,"request":request,
            "coordinator":common::binding(&executable).unwrap()
        }),
    );
    let process =
        json!({"pid":redis["pid"],"started":redis["started"],"executable":redis["executable"]});
    let receipt = save(
        &output.join("process.json"),
        &json!({"launch":launch,"supervisor":supervisor,"process":process}),
    );
    let actual = json!({"bind":"127.0.0.1","port":REDIS_PORT.to_string(),
        "dir":cygwin_path(request["directory"]["path"].as_str().unwrap()).unwrap(),
        "databases":"1","protected-mode":"yes","save":"","appendonly":"no","daemonize":"no"});
    let facts = json!({"process_id":redis["pid"].as_u64().unwrap().to_string(),"run_id":redis["run_id"],
        "config_file":cygwin_path(request["configuration"]["path"].as_str().unwrap()).unwrap(),
        "configuration_sha256":digest::plan_hash(&actual).unwrap()});
    json!({"format_version":3,"request_sha256":digest::plan_hash(request).unwrap(),
        "output":output,"launch":launch,"supervisor":supervisor,"process":process,
        "process_receipt":receipt,"facts":facts,"redis":redis})
}

fn running(
    f: &Fixture,
    run: &Path,
    root: &Path,
    request: &Value,
    redis_request: &Value,
) -> (Value, Value) {
    let mut identity = f.request["storage"]["rustfs"]["identity"].clone();
    identity["pid"] = json!(43);
    identity["started"] = json!("12346");
    let mut process = crate::recovery::clone::native::json::read(Path::new(
        f.request["storage"]["rustfs"]["process_receipt"]["path"]
            .as_str()
            .unwrap(),
    ))
    .unwrap();
    process["identity"] = identity.clone();
    let mut launch = crate::recovery::clone::native::json::read(Path::new(
        f.request["storage"]["rustfs"]["launch_receipt"]["path"]
            .as_str()
            .unwrap(),
    ))
    .unwrap();
    launch["identity"] = identity.clone();
    let process = save(&root.join("rustfs/process.json"), &process);
    let launch = save(&root.join("rustfs/launch.json"), &launch);
    let tree = save(
        &root.join("rustfs/rustfs-tree.json"),
        &json!({"format_version":2,"process":identity}),
    );
    let request = save(&root.join("rustfs/request.json"), request);
    let storage = json!({"request":request,"storage":{"identity":identity,"sha256":f.request["storage"]["rustfs"]["sha256"],
        "process_receipt":process,"launch_receipt":launch,"tree":tree}});
    let mut request = redis_request.clone();
    let windows = request["format_version"] == 3;
    if windows {
        let previous = document::read(&run.join("redis/runtime.json")).unwrap();
        request["previous_identity"] = previous["process"].clone();
    } else {
        request["previous_identity"] = json!({"pid":200,"started":"67890"});
        request["previous_boot_id"] = json!("fixture-boot");
    }
    request["previous_run_id"] = f.request["storage"]["redis"]["run_id"].clone();
    let output = root.join("redis");
    let request_binding = save(&output.join("request.json"), &request);
    let mut redis = f.request["storage"]["redis"].clone();
    redis["pid"] = json!(201);
    redis["started"] = json!("67891");
    redis["run_id"] = json!("4".repeat(40));
    let runtime = if windows {
        windows_runtime(f, &output, &request, &redis)
    } else {
        json!({"output":output,"redis":redis})
    };
    save(&output.join("runtime.json"), &runtime);
    (
        storage,
        json!({"request":request_binding,"runtime":runtime}),
    )
}
