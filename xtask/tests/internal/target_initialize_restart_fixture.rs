//! 登记首代、正常关闭和完整restart结果；只准备本地历史，不启动服务。
use super::*;
use crate::recovery::{
    clone::native::{
        lifecycle::{lock::RunLock, state},
        resources::common,
    },
    fixture_control::services::history,
};
use std::path::Path;

pub(super) struct Restart {
    pub(super) storage: Value,
    pub(super) cache: Value,
}

pub(super) fn register(f: &Fixture) -> Restart {
    let run = f.local.join("registered-restart");
    fs::create_dir(&run).unwrap();
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
    state::initialize(&run).unwrap();
    for (stage, mode, value) in [
        (
            "storage-target",
            "initial",
            f.request["storage"]["rustfs"].clone(),
        ),
        (
            "cache-target",
            "initial",
            json!({"runtime":{"redis":f.request["storage"]["redis"]}}),
        ),
        (
            "fixture-buckets",
            "prepare",
            json!({"status":"buckets_ready"}),
        ),
    ] {
        let (number, _) = state::begin(&run, stage, mode, &json!({})).unwrap();
        state::finish(&run, number, Some(&value), None).unwrap();
    }
    let storage_request = json!({"data_directory":directory(f),
        "api_url":"http://127.0.0.1:29000","console_url":"http://127.0.0.1:29001"});
    save(&run.join("rustfs/request.json"), &storage_request);
    let redis_request = json!({"scope_id":SCOPE,"configuration":f.request["storage"]["redis"]["configuration"],
        "previous_identity":null,"previous_boot_id":null,"previous_run_id":null});
    save(&run.join("redis/request.json"), &redis_request);
    let previous = close(&run);
    let sources = json!({"manifest":common::binding(&run.join("manifest.json")).unwrap(),
        "state_before":common::binding(&run.join("state.json")).unwrap(),"restart_predecessor":previous,"review":review});
    let owner = RunLock::claim(&run).unwrap();
    let (number, attempt) = state::begin(&run, "fixture-services", "restart", &sources).unwrap();
    state::bind_controller(&run, number, &owner.owner, &attempt).unwrap();
    let controller = common::binding(&state::controller_path(&run, number)).unwrap();
    let root = run.join(format!("lifecycle-{number:04}"));
    let (storage, redis) = running(f, &root, &storage_request, &redis_request);
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
    state::finish(&run, number, Some(&result), None).unwrap();
    owner.release().unwrap();
    let actual = history::validate(&run, &state::load(&run, true).unwrap(), true, true).unwrap();
    let bound = actual["active_generation"].clone();
    assert_eq!(
        bound,
        common::binding(&state::result_path(&run, number)).unwrap()
    );
    let mut projection = storage["storage"].clone();
    projection.as_object_mut().unwrap().remove("tree");
    Restart {
        storage: json!({"storage":projection,"data_directory":storage_request["data_directory"],
            "api_url":storage_request["api_url"],"console_url":storage_request["console_url"],"generation":bound}),
        cache: json!({"redis":redis["runtime"]["redis"],"generation":bound}),
    }
}

fn directory(f: &Fixture) -> Value {
    let path = f.local.join("storage/objects");
    let (device, inode) = common::device_inode_for(&path, &fs::metadata(&path).unwrap()).unwrap();
    json!({"path":path,"device":device,"inode":inode})
}

fn save(path: &Path, value: &Value) -> Value {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    crate::recovery::clone::native::json::write_new(path, value).unwrap();
    common::binding(path).unwrap()
}

fn close(run: &Path) -> Value {
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
    let redis = save(
        &root.join("stopped.json"),
        &json!({"status":"redis_process_stopped","alive":false,"resources_deleted":false}),
    );
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

fn running(f: &Fixture, root: &Path, request: &Value, redis_request: &Value) -> (Value, Value) {
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
    request["previous_identity"] = json!({"pid":200,"started":"67890"});
    request["previous_boot_id"] = json!("fixture-boot");
    request["previous_run_id"] = f.request["storage"]["redis"]["run_id"].clone();
    let request = save(&root.join("redis/request.json"), &request);
    let mut redis = f.request["storage"]["redis"].clone();
    redis["pid"] = json!(201);
    redis["started"] = json!("67891");
    redis["run_id"] = json!("4".repeat(40));
    let runtime = json!({"output":root.join("redis"),"redis":redis});
    save(&root.join("redis/runtime.json"), &runtime);
    (storage, json!({"request":request,"runtime":runtime}))
}
