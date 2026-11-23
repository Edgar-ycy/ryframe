//! 五桶逐项独占 create+HEAD；远端写入前记录意图，任何未知结果禁止重放。
use super::*;
use crate::recovery::clone::native::{
    input::BUCKETS, s3::app_configuration, target_verify::input::request_review::Kind,
};
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
    let side = Kind::read(&inputs.review)?.service_side();
    let result = create_all(
        output,
        &inputs.review["scopes"][side]["scope_id"],
        &inputs.configuration,
        || guard(inputs, lock, &sources, &predecessors, &running),
        |operation, request| invoke(inputs, operation, request),
    );
    finish(inputs, number, result, "NativeBucketInitializationFailed")
}

fn create_all(
    output: &Path,
    scope_id: &Value,
    configuration_binding: &Value,
    mut guard: impl FnMut() -> Result<()>,
    mut invoke: impl FnMut(&str, &Value) -> Result<Value>,
) -> Result<Value> {
    let mut receipts = Vec::new();
    let mut buckets = BUCKETS.to_vec();
    buckets.sort_unstable();
    for bucket in &buckets {
        guard()?;
        let request = json!({"format_version":1,"kind":"ryframe-native-s3-create-bucket",
            "scope_id":scope_id,"configuration_binding":configuration_binding,"bucket":bucket});
        document::write_new(&output.join(format!("intent-{bucket}.json")), &request)?;
        let outcome = invoke("__s3-create-bucket", &request)?;
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
        guard()?;
        let mut request = request;
        request["kind"] = json!("ryframe-native-s3-inspect");
        request["operation"] = json!("head-bucket");
        request["prefix"] = Value::Null;
        request["continuation"] = Value::Null;
        let observed = invoke("__s3-inspect", &request)?;
        let head = output.join(format!("head-{bucket}.json"));
        document::write_new(
            &head,
            &json!({"transport":"rust-s3-sigv4","operation":"head-bucket",
            "bucket":bucket,"response":observed,"returncode":0,"error_type":null,"remote_writes":0}),
        )?;
        check(observed == json!({}), "对象桶 HEAD 返回未识别字段")?;
        receipts.push(json!({"bucket":bucket,"create":common::binding(&create)?,"head":common::binding(&head)?}));
        guard()?;
    }
    Ok(
        json!({"status":"fixture_buckets_ready","buckets":buckets,"receipts":receipts,
        "transport":"rust-s3-sigv4","remote_writes":BUCKETS.len()}),
    )
}

fn configuration(inputs: &Inputs) -> Result<Value> {
    let settings = inputs.settings()?;
    let side = Kind::read(&inputs.review)?.service_side();
    let selected = &inputs.review["scopes"][side]["objects"];
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

#[cfg(test)]
#[test]
fn bucket_create_preserves_receipts_and_stops_before_head_on_bound_file_drift() {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    let config = directory.path().join("app.test.toml");
    let output = directory.path().join("buckets");
    fs::create_dir(&output).unwrap();
    let initial = "[object_storage]\naccess_key = 'fixture-access-marker'\nsecret_key = 'fixture-secret-marker'\n";
    fs::write(&config, initial).unwrap();
    let snapshot = storage::binding::snapshot(&config).unwrap();
    let configuration =
        app_configuration::configuration_value(&backend, directory.path(), "test").unwrap();
    let scope_id = json!("bucket-drift");
    let intent = json!({"format_version":1,"kind":"ryframe-native-s3-create-bucket",
        "scope_id":scope_id,"configuration_binding":configuration,"bucket":"avatar"});
    let outcome = json!({"status":"created_unverified","http_status":200,
        "remote_write_may_have_occurred":true,"automatic_retry_permitted":false});
    let intent_path = output.join("intent-avatar.json");
    let create_path = output.join("create-avatar.json");
    let mut checks = 0;
    let mut calls = 0;
    let error = create_all(
        &output,
        &scope_id,
        &configuration,
        || {
            checks += 1;
            storage::binding::verify_snapshot(&snapshot)
        },
        |operation, request| {
            calls += 1;
            assert_eq!(calls, 1);
            assert_eq!(operation, "__s3-create-bucket");
            assert_eq!(request, &intent);
            assert_eq!(document::read(&intent_path).unwrap(), intent);
            fs::write(&config, format!("{initial}# drift\n"))?;
            Ok(outcome.clone())
        },
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "RustFS 输入字节或内核文件身份变化");
    assert_eq!((checks, calls), (2, 1));
    assert_eq!(fs::read_dir(&output).unwrap().count(), 2);
    assert_eq!(document::read(&intent_path).unwrap(), intent);
    assert_eq!(
        document::read(&create_path).unwrap(),
        json!({"transport":"rust-s3-sigv4","operation":"create-bucket","bucket":"avatar",
            "outcome":outcome,"returncode":0,"error_type":null})
    );
    for path in [intent_path, create_path] {
        let receipt = fs::read_to_string(path).unwrap();
        assert!(
            !receipt.contains("fixture-access-marker")
                && !receipt.contains("fixture-secret-marker")
        );
    }
}

#[cfg(test)]
#[test]
fn bucket_credentials_require_direct_values_in_the_bound_toml() {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::tempdir_in(local).unwrap();
    let selected = directory.path().join("app.test.toml");
    let endpoint = "[object_storage]\nendpoint='http://127.0.0.1:29200'\nregion='us-east-1'\n";
    let access = "access_key='file-access-marker'\n";
    let secret = "secret_key='file-secret-marker'\n";
    let read = |access: &str, secret: &str| {
        fs::write(&selected, format!("{endpoint}{access}{secret}")).unwrap();
        let binding =
            app_configuration::configuration_value(&backend, directory.path(), "test").unwrap();
        app_configuration::private_configuration(&backend, &binding)
    };
    let valid = read(access, secret).unwrap();
    assert_eq!(valid["access_key"], "file-access-marker");
    assert_eq!(valid["secret_key"], "file-secret-marker");
    for (access, secret) in [
        ("", secret),
        ("access_key=''\n", secret),
        ("access_key=123\n", secret),
        (access, ""),
        (access, "secret_key=''\n"),
        (access, "secret_key=123\n"),
        (
            "access_key_env='AWS_ACCESS_KEY_ID'\n",
            "secret_key_env='AWS_SECRET_ACCESS_KEY'\n",
        ),
    ] {
        let error = read(access, secret).unwrap_err().to_string();
        assert!(error.contains("object_storage"), "{error}");
        assert!(!error.contains("file-access-marker") && !error.contains("file-secret-marker"));
    }
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
