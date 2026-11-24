//! 复核 Windows Redis 失败收据与原 Job 清空事实。
use super::*;
use crate::recovery::{
    clone::native::{cache::host_control, target_verify::authority},
    runtime::dataset_authority::reads::history as history_reads,
};

const REQUIRED: &[&str] = &["request.json", LAUNCH, "intent.json", "failed.json"];
const OPTIONAL: &[&str] = &[
    "authorized.json",
    "process.json",
    "acknowledged.json",
    "accepted.json",
    "runtime.json",
    "cancel.json",
    "exited.json",
];
const LOGS: &[&str] = &["redis.stdout.log", "redis.stderr.log"];

pub(in crate::recovery::clone::native::cache) fn read(
    request: &Value,
    output: &Path,
    live: bool,
) -> Result<Value> {
    read_failure(&crate::workspace::root_dir(), request, output, live, None)
}

pub(in crate::recovery::clone::native::cache) fn accepted_failure(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    live: bool,
) -> Result<Value> {
    let output = bound_path(runtime, "output")?;
    read_failure(backend, request, &output, live, Some(runtime))
}

fn read_failure(
    backend: &Path,
    request: &Value,
    output: &Path,
    live: bool,
    published: Option<&Value>,
) -> Result<Value> {
    common::local_directory(backend, output, "Windows Redis 失败目录")?;
    let bindings = files(output)?;
    let protected = ProtectedInputs::open(
        bindings
            .values()
            .map(|value| bound_path(value, "path"))
            .collect::<Result<Vec<_>>>()?,
    )?;
    let result = (|| {
        let launch = BoundLaunch::load_frozen(
            &output.join(LAUNCH),
            document::text(
                &bindings["redis_launch-request.json"]["sha256"],
                "原启动摘要",
            )?,
        )?;
        history_reads::directory(&launch.backend)?;
        if published.is_some()
            && std::fs::canonicalize(backend)? != std::fs::canonicalize(&launch.backend)?
        {
            return Err("Windows Redis 已发布失败收据属于其他协调工作区".into());
        }
        let records = Records {
            output,
            bindings: &bindings,
        };
        if launch.request != *request
            || records.required("request.json")? != *request
            || records.required("intent.json")? != launch.binding
            || records
                .optional("cancel.json")?
                .is_some_and(|value| value != launch.binding)
        {
            return Err("Windows Redis 失败收据不属于原请求、启动意图或取消".into());
        }
        if let Some(runtime) = published
            && records.required("failed.json")?["cleanup_completed"] == false
        {
            let proof =
                super::observe::incident::incomplete_cleanup(backend, request, runtime, live)?;
            if files(output)? != bindings {
                return Err("Windows Redis 事故目录证据在核验期间变化".into());
            }
            return Ok(proof);
        }
        let proof = evidence(&launch, &records)?;
        let runtime = published
            .map(|runtime| require_published(&launch, &records, runtime))
            .transpose()?;
        if live {
            let launch_inputs = launch.protect()?;
            launch_inputs.finish(observe_closed(request, &proof))?;
        }
        if files(output)? != bindings {
            return Err("Windows Redis 失败目录证据在核验期间变化".into());
        }
        if let Some(runtime) = runtime {
            return Ok(json!({"kind":"windows-redis-accepted-failure",
                "runtime":runtime,"bindings":bindings,"windows_closed":proof}));
        }
        Ok(json!({"runtime":null,"bindings":bindings,"windows_closed":proof}))
    })();
    protected.finish(result)
}

fn require_published<'a>(
    launch: &BoundLaunch,
    records: &'a Records<'_>,
    runtime: &Value,
) -> Result<&'a Value> {
    document::exact(
        runtime,
        &[
            "format_version",
            "request_sha256",
            "output",
            "launch",
            "supervisor",
            "process",
            "process_receipt",
            "facts",
            "redis",
        ],
        "Windows Redis 已发布运行收据",
    )?;
    document::exact(
        &runtime["facts"],
        &[
            "process_id",
            "run_id",
            "config_file",
            "configuration_sha256",
        ],
        "Windows Redis 已发布认证事实",
    )?;
    let request = &launch.request;
    let process = &runtime["process"];
    if runtime["format_version"] != 3
        || records.binding("cancel.json").is_some()
        || records.required("runtime.json")? != *runtime
        || runtime["redis"]
            != json!({
                "platform":"windows","port":request["port"],"pid":process["pid"],
                "started":process["started"],"executable":request["tool"]["path"],
                "sha256":request["tool"]["sha256"],"configuration":request["configuration"],
                "run_id":runtime["facts"]["run_id"],"tool":request["tool"],
            })
    {
        return Err("Windows Redis 失败收据不属于原非取消的已发布运行代次".into());
    }
    records
        .binding("runtime.json")
        .ok_or_else(|| "Windows Redis 缺少已发布运行收据绑定".into())
}

fn evidence(launch: &BoundLaunch, records: &Records<'_>) -> Result<Value> {
    let failure = records.required("failed.json")?;
    document::exact(
        &failure,
        &[
            "launch",
            "supervisor",
            "spawn_attempted",
            "cleanup_completed",
        ],
        "Windows Redis 失败收据",
    )?;
    let supervisor = &failure["supervisor"];
    model::identity(supervisor, false)?;
    if failure["launch"] != launch.binding
        || failure["cleanup_completed"] != true
        || !same_path(
            document::text(&supervisor["executable"], "原监督器映像")?,
            document::text(&launch.coordinator["path"], "原协调工具")?,
        )
    {
        return Err("Windows Redis 原监督器或 Job 清理未确认".into());
    }
    let authorized = records.optional("authorized.json")?;
    let expected = json!({"launch":launch.binding,"supervisor":supervisor});
    if authorized.as_ref().is_some_and(|value| value != &expected) {
        return Err("Windows Redis 失败收据未绑定原授权".into());
    }
    let (process, exit) = match failure["spawn_attempted"].as_bool() {
        Some(true) if authorized.is_some() => process_exit(launch, records, supervisor)?,
        Some(false)
            if [
                "process.json",
                "acknowledged.json",
                "accepted.json",
                "runtime.json",
                "exited.json",
            ]
            .iter()
            .all(|name| records.binding(name).is_none()) =>
        {
            (Value::Null, Value::Null)
        }
        _ => return Err("Windows Redis 失败收据无法确认是否已创建进程".into()),
    };
    Ok(
        json!({"state":"stopped","launch":launch.binding,"supervisor":supervisor,
        "process":process,"exit":exit,"failure":records.binding("failed.json")}),
    )
}

fn process_exit(
    launch: &BoundLaunch,
    records: &Records<'_>,
    supervisor: &Value,
) -> Result<(Value, Value)> {
    let record = records.required("process.json")?;
    document::exact(
        &record,
        &["launch", "supervisor", "process"],
        "原 Redis 进程记录",
    )?;
    let process = &record["process"];
    model::identity(process, false)?;
    let binding = records
        .binding("process.json")
        .ok_or("缺少 Redis 原进程绑定")?;
    let exit_binding = records
        .binding("exited.json")
        .ok_or("缺少 Redis 原退出绑定")?;
    let exited = records.required("exited.json")?;
    document::exact(
        &exited,
        &["process", "returncode", "job_empty"],
        "原 Redis 退出记录",
    )?;
    if record["launch"] != launch.binding
        || record["supervisor"] != *supervisor
        || process["pid"] == supervisor["pid"]
        || !same_path(
            document::text(&process["executable"], "原 Redis 映像")?,
            document::text(&launch.request["tool"]["path"], "原 Redis 程序")?,
        )
        || exited["process"] != *binding
        || exited["returncode"]
            .as_i64()
            .is_none_or(|code| i32::try_from(code).is_err())
        || exited["job_empty"] != true
    {
        return Err("Windows Redis 退出未证明原进程及原 Job 已清空".into());
    }
    accepted(launch, records, supervisor, process, binding)?;
    Ok((process.clone(), exit_binding.clone()))
}

fn accepted(
    launch: &BoundLaunch,
    records: &Records<'_>,
    supervisor: &Value,
    process: &Value,
    binding: &Value,
) -> Result<()> {
    let ack = records.optional("acknowledged.json")?;
    if let Some(value) = &ack {
        document::exact(value, &["process", "run_id"], "原 Redis 确认")?;
        if value["process"] != *binding || !model::matches(&value["run_id"], r"^[a-f0-9]{40}$")? {
            return Err("Windows Redis 失败路径包含不属于原进程的确认".into());
        }
    }
    let accepted = records.optional("accepted.json")?;
    if accepted.is_some() && accepted != ack {
        return Err("Windows Redis 接受与原进程确认不同".into());
    }
    if let Some(runtime) = records.optional("runtime.json")? {
        let confirmation = accepted.ok_or("失败路径运行收据缺少原接受")?;
        if runtime["format_version"] != 3
            || runtime["request_sha256"] != digest::plan_hash(&launch.request)?
            || bound_path(&runtime, "output")? != launch.output
            || runtime["launch"] != launch.binding
            || runtime["process"] != *process
            || runtime["supervisor"] != *supervisor
            || runtime["process_receipt"] != *binding
            || runtime["facts"]["run_id"] != confirmation["run_id"]
            || runtime["redis"]["run_id"] != confirmation["run_id"]
        {
            return Err("Windows Redis 失败路径的运行收据不属于原接受代次".into());
        }
        super::super::facts::check(
            &launch.request,
            process,
            &runtime["facts"],
            &confirmation["run_id"],
        )?;
    }
    Ok(())
}

fn observe_closed(request: &Value, proof: &Value) -> Result<()> {
    let stopped = || {
        if super::observe::present(&proof["supervisor"])?
            || !proof["process"].is_null() && super::observe::present(&proof["process"])?
        {
            return Err("Windows Redis 原失败代次仍在运行".into());
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let url = format!(
        "http://127.0.0.1:{}",
        request["port"].as_u64().ok_or("Redis 端口无效")?
    );
    stopped()?;
    authority::require_closed(&url)?;
    host_control::bind_and_close(request)?;
    authority::require_closed(&url)?;
    stopped()
}

type Bindings = serde_json::Map<String, Value>;

fn files(output: &Path) -> Result<Bindings> {
    let mut result = Bindings::new();
    for entry in history_reads::entries(output)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or("Windows Redis 失败目录文件名不是 UTF-8")?;
        let path = model::plain(&entry.path())?;
        if !entry.file_type()?.is_file()
            || ![REQUIRED, OPTIONAL, LOGS]
                .iter()
                .any(|names| names.contains(&name))
        {
            return Err("Windows Redis 失败目录含未知文件、链接或子目录".into());
        }
        if !LOGS.contains(&name) {
            result.insert(format!("redis_{name}"), common::binding(&path)?);
        }
    }
    if REQUIRED
        .iter()
        .any(|name| !result.contains_key(&format!("redis_{name}")))
    {
        return Err("Windows Redis 失败目录缺少原请求、意图或清理终结收据".into());
    }
    Ok(result)
}

struct Records<'a> {
    output: &'a Path,
    bindings: &'a Bindings,
}

impl Records<'_> {
    fn binding(&self, name: &str) -> Option<&Value> {
        self.bindings.get(&format!("redis_{name}"))
    }

    fn required(&self, name: &str) -> Result<Value> {
        self.optional(name)?
            .ok_or_else(|| "Windows Redis 失败路径缺少原收据".into())
    }

    fn optional(&self, name: &str) -> Result<Option<Value>> {
        self.binding(name)
            .map(|_| document::read(&self.output.join(name)))
            .transpose()
    }
}

#[cfg(test)]
#[test]
fn first_failure_binds_original_windows_job_and_never_accepts_unknown_spawn() {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let root = tempfile::tempdir_in(local).unwrap();
    let output = root.path();
    let mut request = model::tests::windows_request();
    request["previous_identity"] = Value::Null;
    request["previous_run_id"] = Value::Null;
    let supervisor = json!({"pid":31,"started":"231","executable":"D:/test/xtask.exe"});
    let process = json!({"pid":32,"started":"232","executable":request["tool"]["path"]});
    let record = |name: &str, value: &Value| {
        std::fs::write(output.join(name), serde_json::to_vec(value).unwrap()).unwrap();
    };
    record("request.json", &request);
    record(
        LAUNCH,
        &json!({"format_version":3,"backend":backend,"output":output,
        "owner":{"pid":30,"started":"230","executable":"D:/test/xtask.exe"},
        "coordinator":{"path":"D:/test/xtask.exe","bytes":7,"sha256":"1".repeat(64)},
        "request":request}),
    );
    let launch = common::binding(&output.join(LAUNCH)).unwrap();
    record("intent.json", &launch);
    let mut failure = json!({"launch":launch,"supervisor":supervisor,
        "spawn_attempted":false,"cleanup_completed":true});
    record("failed.json", &failure);
    assert!(read(&request, output, false).unwrap()["windows_closed"]["process"].is_null());

    failure["spawn_attempted"] = json!(true);
    record("failed.json", &failure);
    record(
        "authorized.json",
        &json!({"launch":launch,"supervisor":supervisor}),
    );
    assert!(read(&request, output, false).is_err());

    record(
        "process.json",
        &json!({"launch":launch,"supervisor":supervisor,"process":process}),
    );
    let original = common::binding(&output.join("process.json")).unwrap();
    let mut exited = json!({"process":original,"returncode":1,"job_empty":false});
    record("exited.json", &exited);
    assert!(read(&request, output, false).is_err());
    exited["job_empty"] = json!(true);
    record("exited.json", &exited);
    let confirmed = read(&request, output, false).unwrap();
    assert!(confirmed["runtime"].is_null());
    assert_eq!(confirmed["windows_closed"]["process"], process);

    let known_failure =
        |runtime: &Value| host_control::accepted_failure(&backend, &request, runtime, false);
    let run_id = "b".repeat(40);
    let confirmation = json!({"process":original,"run_id":run_id});
    let runtime = json!({
        "format_version":3,"request_sha256":digest::plan_hash(&request).unwrap(),
        "output":output,"launch":launch,"supervisor":supervisor,"process":process,
        "process_receipt":original,"facts":{
            "process_id":"216","run_id":run_id,"config_file":model::configuration_file(&request).unwrap(),
            "configuration_sha256":digest::plan_hash(&model::configuration(&request).unwrap()).unwrap(),
        },
        "redis":{
            "platform":"windows","port":request["port"],"pid":process["pid"],
            "started":process["started"],"executable":request["tool"]["path"],
            "sha256":request["tool"]["sha256"],"configuration":request["configuration"],
            "run_id":run_id,"tool":request["tool"],
        },
    });
    // 只有历史首代的清空收据时，不能作为已接受、已发布代次的失败证明。
    assert!(known_failure(&runtime).is_err());
    record("acknowledged.json", &confirmation);
    record("runtime.json", &runtime);
    assert!(known_failure(&runtime).is_err());
    record("accepted.json", &confirmation);
    let accepted = known_failure(&runtime).unwrap();
    let verified = read(&request, output, false).unwrap();
    assert_eq!(
        accepted,
        json!({"kind":"windows-redis-accepted-failure",
            "runtime":common::binding(&output.join("runtime.json")).unwrap(),
            "bindings":verified["bindings"],"windows_closed":verified["windows_closed"]})
    );
    let mut different = runtime.clone();
    different["redis"]["pid"] = json!(33);
    assert!(known_failure(&different).is_err());
    record("cancel.json", &launch);
    assert!(known_failure(&runtime).is_err());
    assert!(read(&request, output, false).is_ok());
    std::fs::remove_file(output.join("cancel.json")).unwrap();

    failure["cleanup_completed"] = json!(false);
    record("failed.json", &failure);
    assert!(read(&request, output, false).is_err());
    assert!(known_failure(&runtime).is_err());
}
