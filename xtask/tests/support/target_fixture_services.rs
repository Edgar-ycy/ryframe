//! 两个测试入口共用的原存储文件和关闭历史夹具。
use std::{fs, path::Path};

use serde_json::{Value, json};

use crate::recovery::{
    clone::native::{
        cache::model::render_configuration, digest, json as document, lifecycle::state,
        resources::common,
    },
    fixture_control::services::history,
};

pub const SCOPE: &str = "seed-scope";
pub const REDIS_PORT: u16 = 26379;
pub const REDIS_PASSWORD: &str = "fixture-redis";

pub fn windows_redis_files(local: &Path, redis: &Path) -> Value {
    let package = local.join("redis-package");
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
    let package = json!({"platform":"windows","path":program,"sha256":digest::file(&program).unwrap().sha256,
        "version":"Redis server v=7.4.11 bits=64","dlls":{"msys-2.0.dll":digest::file(&dll).unwrap().sha256}});
    json!({"platform":"windows","port":REDIS_PORT,"pid":200,"started":"67890","executable":program,
        "sha256":package["sha256"],"run_id":"3".repeat(40),
        "configuration":common::binding(&configuration).unwrap(),"tool":package})
}

pub fn save(path: &Path, value: &Value) -> Value {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    crate::recovery::clone::native::json::write_new(path, value).unwrap();
    common::binding(path).unwrap()
}

pub fn close(run: &Path) -> Value {
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
