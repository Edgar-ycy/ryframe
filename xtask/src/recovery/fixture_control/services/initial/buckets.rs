//! 五桶逐项独占 create+HEAD；远端写入前记录意图，任何未知结果禁止重放。
use super::*;
use crate::recovery::clone::native::{input::BUCKETS, s3::app_configuration};
use crate::recovery::fixture_control::services::lifecycle::{Runtime, readonly::NativeRuntime};
use std::time::Duration;

const PREFIX: &[(&str, &str)] = &[("storage-target", "initial"), ("cache-target", "initial")];

pub(super) fn run(inputs: &Inputs) -> Result<Value> {
    prefix(inputs, PREFIX)?;
    let output = inputs.run.join("buckets");
    check(
        !output.try_exists()?,
        "对象桶首代输出已经存在，禁止覆盖或重放",
    )?;
    let _guard = ProcessGuard::acquire(&inputs.run)?;
    let lock = RunLock::claim(&inputs.run)?;
    let result = execute(inputs, &lock, &output);
    let released = lock.release();
    let result = result?;
    released?;
    Ok(result)
}

fn execute(inputs: &Inputs, lock: &RunLock, output: &Path) -> Result<Value> {
    let predecessors = predecessors(inputs)?;
    observe_services(inputs, &predecessors)?;
    inputs.guard()?;
    configuration(inputs)?;
    common::ensure_new_directory(&inputs.backend, output, "五桶首代输出")?;
    let mut sources = inputs.sources.clone();
    sources["manifest"] = common::binding(&inputs.run.join("manifest.json"))?;
    sources["configuration_sha256"] = json!(digest::plan_hash(&configuration(inputs)?)?);
    sources["predecessors"] = predecessors["bindings"].clone();
    let (number, attempt) = state::begin(&inputs.run, "fixture-buckets", "prepare", &sources)?;
    state::bind_controller(&inputs.run, number, &lock.owner, &attempt)?;
    let running = json!({"ledger":storage::binding::snapshot(&inputs.run.join("state.json"))?,
        "controller":storage::binding::snapshot(&state::controller_path(&inputs.run, number))?});
    let result = create_all(inputs, lock, output, &sources, &predecessors, &running);
    finish(inputs, number, result, "NativeBucketInitializationFailed")
}

fn create_all(
    inputs: &Inputs,
    lock: &RunLock,
    output: &Path,
    sources: &Value,
    predecessors: &Value,
    running: &Value,
) -> Result<Value> {
    let mut receipts = Vec::new();
    let mut buckets = BUCKETS.to_vec();
    buckets.sort_unstable();
    for bucket in &buckets {
        guard(inputs, lock, sources, predecessors, running)?;
        let request = json!({"format_version":1,"kind":"ryframe-native-s3-create-bucket",
            "scope_id":inputs.review["scopes"]["seed"]["scope_id"],
            "configuration_binding":inputs.configuration,"bucket":bucket});
        document::write_new(&output.join(format!("intent-{bucket}.json")), &request)?;
        let outcome = invoke(inputs, "__s3-create-bucket", &request)?;
        let create = output.join(format!("create-{bucket}.json"));
        document::write_new(
            &create,
            &json!({"transport":"rust-s3-sigv4","operation":"create-bucket",
            "bucket":bucket,"outcome":outcome,"returncode":0,"error_type":null}),
        )?;
        check(
            outcome["status"] == "created_unverified",
            "对象桶创建未获得可继续核验的新建结果；禁止接管或重试",
        )?;
        guard(inputs, lock, sources, predecessors, running)?;
        let mut request = request;
        request["kind"] = json!("ryframe-native-s3-inspect");
        request["operation"] = json!("head-bucket");
        request["prefix"] = Value::Null;
        request["continuation"] = Value::Null;
        let observed = invoke(inputs, "__s3-inspect", &request)?;
        let head = output.join(format!("head-{bucket}.json"));
        document::write_new(
            &head,
            &json!({"transport":"rust-s3-sigv4","operation":"head-bucket",
            "bucket":bucket,"response":observed,"returncode":0,"error_type":null,"remote_writes":0}),
        )?;
        check(observed == json!({}), "对象桶 HEAD 返回未识别字段")?;
        receipts.push(json!({"bucket":bucket,"create":common::binding(&create)?,"head":common::binding(&head)?}));
        guard(inputs, lock, sources, predecessors, running)?;
    }
    Ok(
        json!({"status":"fixture_buckets_ready","buckets":buckets,"receipts":receipts,
        "transport":"rust-s3-sigv4","remote_writes":BUCKETS.len()}),
    )
}

fn configuration(inputs: &Inputs) -> Result<Value> {
    let settings = inputs.settings()?;
    let selected = &inputs.review["scopes"]["seed"]["objects"];
    let value = json!({"endpoint":selected["endpoint"],"region":selected["region"],
        "access_key":settings["object_storage"]["access_key"],"secret_key":settings["object_storage"]["secret_key"]});
    check(
        value == app_configuration::private_configuration(&inputs.backend, &inputs.configuration)?,
        "对象桶连接身份与绑定配置文件不同",
    )?;
    Ok(value)
}

fn guard(
    inputs: &Inputs,
    lock: &RunLock,
    sources: &Value,
    predecessors: &Value,
    running: &Value,
) -> Result<()> {
    lock.verify()?;
    inputs.guard()?;
    common::bound_file(&inputs.backend, &sources["manifest"], "五桶固定服务清单")?;
    check(
        digest::plan_hash(&configuration(inputs)?)? == sources["configuration_sha256"],
        "对象桶初始化期间绑定配置发生变化",
    )?;
    for key in ["ledger", "controller"] {
        storage::binding::verify_snapshot(&running[key])?;
    }
    running_prefix(&state::load(&inputs.run, true)?, &predecessors["state"])?;
    for snapshot in predecessors["snapshots"]
        .as_array()
        .ok_or("对象桶缺少原前驱文件身份")?
    {
        storage::binding::verify_snapshot(snapshot)?;
    }
    observe_services(inputs, predecessors)
}

fn predecessors(inputs: &Inputs) -> Result<Value> {
    let saved = prefix(inputs, PREFIX)?;
    let mut bindings = json!({});
    for (index, role) in ["rustfs", "redis"].iter().enumerate() {
        bindings[format!("{role}_request")] =
            saved["attempts"][index]["sources"]["request"].clone();
        bindings[format!("{role}_result")] = saved["attempts"][index]["result"].clone();
    }
    let storage = common::read_bound(
        &inputs.backend,
        &bindings["rustfs_result"],
        "原成功 RustFS 前驱",
    )?;
    bindings["rustfs_tree"] = storage["tree"].clone();
    let cache = common::read_bound(
        &inputs.backend,
        &bindings["redis_result"],
        "原成功 Redis 前驱",
    )?;
    bindings["redis_runtime"] = common::binding(&inputs.run.join("redis/runtime.json"))?;
    check(
        common::read_bound(
            &inputs.backend,
            &bindings["redis_runtime"],
            "原 Redis runtime",
        )? == cache["runtime"],
        "原 Redis 成功结果与 runtime 不一致",
    )?;
    let mut snapshots = Vec::new();
    for descriptor in bindings.as_object().ok_or("对象桶前驱绑定无效")?.values() {
        let path = common::bound_file(&inputs.backend, descriptor, "对象桶原前驱")?;
        snapshots.push(storage::binding::snapshot(&path)?);
    }
    Ok(
        json!({"state":saved,"bindings":bindings,"snapshots":snapshots,
        "redis_request":common::read_bound(&inputs.backend,&bindings["redis_request"],"原 Redis 请求")?,
        "redis_runtime":cache["runtime"]}),
    )
}

fn observe_services(inputs: &Inputs, predecessors: &Value) -> Result<()> {
    super::redis::storage_running(inputs, &predecessors["state"])?;
    let mut runtime = NativeRuntime::new(&inputs.backend);
    check(
        runtime.call(
            "cache-status",
            json!({"request":predecessors["redis_request"],
        "runtime":predecessors["redis_runtime"],"effective":null}),
        )?["state"]
            == "running",
        "对象桶写入前后原 Redis 实际身份或运行状态变化",
    )
}

fn running_prefix(current: &Value, previous: &Value) -> Result<()> {
    let attempts = state::attempts(current)?;
    let original = state::attempts(previous)?;
    check(
        original.len() == 2
            && attempts.len() == 3
            && attempts[..2] == *original
            && attempts[2]["number"] == 3
            && attempts[2]["stage"] == "fixture-buckets"
            && attempts[2]["mode"] == "prepare"
            && attempts[2]["status"] == "running",
        "对象桶原服务前缀或当前运行阶段漂移，禁止继续写入",
    )
}

#[cfg(test)]
#[test]
fn bucket_guard_accepts_its_running_third_stage_but_rejects_changed_predecessors_or_continuation() {
    let previous = json!({"attempts":[{"number":1,"stage":"storage-target","mode":"initial","status":"passed"},
        {"number":2,"stage":"cache-target","mode":"initial","status":"passed"}]});
    let mut current = previous.clone();
    current["attempts"]
        .as_array_mut()
        .unwrap()
        .push(json!({"number":3,"stage":"fixture-buckets","mode":"prepare","status":"running"}));
    running_prefix(&current, &previous).unwrap();
    let mut changed = current.clone();
    changed["attempts"][0]["status"] = json!("failed");
    assert!(running_prefix(&changed, &previous).is_err());
    let mut changed = current.clone();
    changed["attempts"][2]["status"] = json!("passed");
    assert!(running_prefix(&changed, &previous).is_err());
    let mut changed = current;
    changed["attempts"]
        .as_array_mut()
        .unwrap()
        .push(json!({"number":4,"status":"running"}));
    assert!(running_prefix(&changed, &previous).is_err());
}

fn invoke(inputs: &Inputs, operation: &str, request: &Value) -> Result<Value> {
    let executable = std::env::current_exe()?;
    let frozen = storage::binding::snapshot(&executable)?;
    let mut command = crate::process::child_command(&executable);
    command.arg(operation).env_clear();
    for (key, value) in std::env::vars_os() {
        if [
            "PATH",
            "PATHEXT",
            "SYSTEMROOT",
            "WINDIR",
            "COMSPEC",
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "TEMP",
            "TMP",
        ]
        .contains(&key.to_string_lossy().to_ascii_uppercase().as_str())
        {
            command.env(key, value);
        }
    }
    let bytes = serde_json::to_vec(request)?;
    check(bytes.len() <= 1024 * 1024, "原生 S3 私有请求超过 1 MiB")?;
    let response = crate::recovery::full_stack::execution::capture(
        command,
        &inputs.backend,
        &bytes,
        Duration::from_secs(1810),
        None,
        false,
        false,
    )
    .map_err(|_| "对象操作未确认；保留意图和前后像，禁止自动重放")?;
    storage::binding::verify_snapshot(&frozen)?;
    check(response.len() <= 1024 * 1024, "原生 S3 私有响应超过 1 MiB")?;
    let value = document::parse_fragment(&response, "原生 S3 响应")?;
    document::exact(&value, &["ok", "result"], "原生 S3 成功响应")?;
    check(
        value["ok"] == true && value["result"].is_object(),
        "原生 S3 响应未确认成功",
    )?;
    Ok(value["result"].clone())
}
