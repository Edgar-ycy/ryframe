use super::*;
use crate::recovery::clone::native::resources::{common, storage};

fn frozen() -> FakeRuntime {
    let mut runtime = FakeRuntime::new();
    let fixture = &mut runtime.fixture;
    let output = fixture.run.join("rustfs");
    let data = fixture.run.join("data");
    fs::create_dir(&data).unwrap();
    let (device, inode) = common::device_inode_for(&data, &fs::metadata(&data).unwrap()).unwrap();
    let identity = json!({"pid":12345,"started":"1","executable":std::env::current_exe().unwrap()});
    let request = json!({"scope_id":"services-fixture-seed","executable":{"path":identity["executable"],"sha256":"1".repeat(64)},
        "data_directory":{"path":data,"device":device,"inode":inode},"credential_files":{},
        "api_url":"http://127.0.0.1:29200","console_url":"http://127.0.0.1:29201"});
    json_io::write_replace(&output.join("request.json"), &request).unwrap();
    let controller = fixture.run.join("controller-0001.json");
    json_io::write_new(&controller, &json!({"fixture":true})).unwrap();
    let manifest = binding(&fixture.run.join("manifest.json")).unwrap();
    json_io::write_new(&output.join("intent.json"), &json!({"format_version":1,"kind":"devex-clone-storage-intent",
        "request":manifest,"controller":binding(&controller).unwrap(),"attempt":1,
        "arguments":storage::model::arguments(&request).unwrap(),"environment":storage::model::configuration(&request).unwrap()})).unwrap();
    let process = json!({"format_version":1,"role":"rustfs","scope_id":request["scope_id"],"lifecycle":"running",
        "identity":identity,"api_url":request["api_url"],"console_url":request["console_url"],
        "data_dir":request["data_directory"]["path"],"intent":binding(&output.join("intent.json")).unwrap()});
    json_io::write_new(&output.join("process.json"), &process).unwrap();
    json_io::write_new(&output.join("launch.json"), &json!({"format_version":1,"scope_id":request["scope_id"],
        "identity":identity,"arguments":storage::model::arguments(&request).unwrap(),
        "environment":storage::model::configuration(&request).unwrap(),"credential_files":request["credential_files"]})).unwrap();
    let mut tree = read(&output.join("rustfs-tree.json")).unwrap();
    tree["process"] = identity.clone();
    json_io::write_replace(&output.join("rustfs-tree.json"), &tree).unwrap();
    let storage = json!({"identity":identity,"sha256":request["executable"]["sha256"],
        "process_receipt":binding(&output.join("process.json")).unwrap(),
        "launch_receipt":binding(&output.join("launch.json")).unwrap(),"tree":binding(&output.join("rustfs-tree.json")).unwrap()});
    let cache =
        json!({"service":"redis","runtime":read(&fixture.run.join("redis/runtime.json")).unwrap()});
    let mut state = fixture.state();
    for (index, role, result) in [(0, "rustfs", storage), (1, "redis", cache)] {
        let path = Path::new(text(&state["attempts"][index]["result"], "path").unwrap());
        json_io::write_replace(path, &result).unwrap();
        state["attempts"][index]["result"] = binding(path).unwrap();
        state["attempts"][index]["sources"]["request"] =
            binding(&fixture.run.join(role).join("request.json")).unwrap();
    }
    json_io::write_replace(&fixture.run.join("state.json"), &state).unwrap();
    runtime
}

fn registered(runtime: &mut FakeRuntime) -> Result<Value> {
    let backend = runtime.fixture.backend.clone();
    let context = runtime.fixture.context();
    registration::load(runtime, &backend, &context, true)
}

#[test]
fn frozen_requests_tree_results_and_launch_evidence_must_agree() {
    let mut runtime = frozen();
    let before = runtime.fixture.state();
    let observed = registered(&mut runtime).unwrap();
    assert_eq!(
        observed["tree"]["process"],
        observed["origin"]["storage"]["identity"]
    );
    assert_eq!(observed["storage_observation"]["state"], "recorded");
    assert!(observed["cache_effective"].is_null());
    assert_eq!(runtime.fixture.state(), before);
    assert_eq!(runtime.events, ["tree-read"]);
    fs::write(runtime.fixture.run.join("redis/request.json"), b"{}").unwrap();
    assert!(registered(&mut runtime).is_err());
}

#[test]
fn historical_records_without_frozen_requests_never_gain_invented_tree_ownership() {
    let mut runtime = FakeRuntime::new();
    let before = runtime.fixture.state();
    assert!(
        registered(&mut runtime)
            .unwrap_err()
            .to_string()
            .contains("历史首代")
    );
    assert_eq!(runtime.fixture.state(), before);
    assert!(runtime.events.is_empty());
}

#[test]
fn request_intent_process_and_launch_drift_fail_without_rewriting_evidence() {
    for filename in [
        "request.json",
        "intent.json",
        "process.json",
        "launch.json",
        "rustfs-tree.json",
    ] {
        let mut runtime = frozen();
        let path = runtime.fixture.run.join("rustfs").join(filename);
        fs::write(&path, b"{}").unwrap();
        let before = fs::read(&path).unwrap();
        assert!(registered(&mut runtime).is_err(), "{filename}");
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn running_callers_keep_their_frozen_prefix_instead_of_reading_new_stage() {
    let mut runtime = frozen();
    let context = runtime.fixture.context();
    let backend = runtime.fixture.backend.clone();
    state::begin(
        &runtime.fixture.run,
        "fixture-services",
        "close",
        &context["sources"],
    )
    .unwrap();
    assert!(registration::load(&mut runtime, &backend, &context, true).is_ok());
    assert!(registered(&mut runtime).is_err());
}

#[test]
fn successor_tools_may_only_change_the_same_launcher_bytes() {
    let mut runtime = frozen();
    let original = json!({"distribution":"fixture","executable":"/usr/bin/redis-server","sha256":"1".repeat(64),
        "python":{"path":"python3","executable":"/usr/bin/python3","sha256":"2".repeat(64)},"wsl":{"path":"old"}});
    json_io::write_replace(&runtime.fixture.run.join("redis/request.json"), &original).unwrap();
    let mut context = runtime.fixture.context();
    let descriptor = binding(&runtime.fixture.run.join("redis/request.json")).unwrap();
    context["state"]["attempts"][1]["sources"]["request"] = descriptor;
    context["sources"]["review"] = json!({"successor":true});
    context["documents"]["review"] = json!({"tools":{"redis_server":{"distribution":"fixture","resolved_path":"/usr/bin/redis-server","sha256":"1".repeat(64)},
        "redis_python":{"distribution":"fixture","path":"python3","resolved_path":"/usr/bin/python3","sha256":"3".repeat(64)},
        "wsl":{"path":"current"}}});
    let backend = runtime.fixture.backend.clone();
    let result = registration::load(&mut runtime, &backend, &context, true).unwrap();
    assert_eq!(result["cache_effective"]["wsl"], json!({"path":"current"}));
    assert_eq!(
        result["cache_effective"]["python"]["sha256"],
        "3".repeat(64)
    );
    context["documents"]["review"]["tools"]["redis_python"]["resolved_path"] =
        json!("/other/python");
    assert!(registration::load(&mut runtime, &backend, &context, true).is_err());
}

#[test]
fn historical_projection_keeps_the_initial_and_published_successor_and_rejects_modified_ledger() {
    let mut runtime = frozen();
    let services = registered(&mut runtime).unwrap();
    let original =
        json!({"rustfs":services["origin"]["storage"],"redis":services["origin"]["redis"]});
    runtime.fixture.services = services;
    runtime.execute("close", None).unwrap();
    let result = runtime.execute("restart", Some(runtime.owner())).unwrap();
    let backend = runtime.fixture.backend.clone();
    let run = runtime.fixture.run.clone();
    let before = fs::read(run.join("state.json")).unwrap();
    runtime.events.clear();
    let values = super::super::projection::historical(&mut runtime, &backend, &run).unwrap();
    assert_eq!(values.as_array().unwrap().len(), 2);
    assert_eq!(values[0]["value"], original);
    assert_eq!(
        values[1]["runtime"]["generation"],
        runtime.fixture.history()["active_generation"]
    );
    assert_ne!(
        values[1]["value"]["rustfs"]["identity"],
        values[0]["value"]["rustfs"]["identity"]
    );
    assert_eq!(fs::read(run.join("state.json")).unwrap(), before);
    assert_eq!(runtime.events, ["context", "registered"]);
    let path = Path::new(text(&result["generation"], "path").unwrap());
    let raw = fs::read(path).unwrap();
    fs::write(path, b"{}").unwrap();
    assert!(super::super::projection::historical(&mut runtime, &backend, &run).is_err());
    assert_eq!(fs::read(path).unwrap(), b"{}");
    fs::write(path, raw).unwrap();
}
