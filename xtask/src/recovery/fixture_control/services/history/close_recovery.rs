//! 失败 close 的新阶段恢复证据；原失败目录及停止收据缺失事实不可改写。
use super::*;
use crate::recovery::{clone::native::resources::common, monitoring::execution::ProtectedInputs};
use std::time::UNIX_EPOCH;

const WINDOWS_REDIS_FILES: &[&str] = &[
    "request.json",
    "launch-request.json",
    "intent.json",
    "authorized.json",
    "process.json",
    "acknowledged.json",
    "accepted.json",
    "runtime.json",
    "exited.json",
];

pub(crate) fn candidate(
    run: &Path,
    failed: &Value,
    generation: &Value,
    sources: &Value,
    services: &Value,
    closed: bool,
) -> Result<Value> {
    let expected = sources.as_object().ok_or("关闭恢复来源无效")?;
    let actual = failed["sources"].as_object().ok_or("原关闭来源无效")?;
    check(
        expected.keys().eq(actual.keys())
            && expected
                .iter()
                .all(|(key, value)| key == "state_before" || actual.get(key) == Some(value))
            && prefix::count(run, &failed["sources"]["state_before"])? + 1
                == failed["number"].as_u64().ok_or("原关闭编号无效")? as usize
            && failed["stage"] == "fixture-services"
            && failed["mode"] == "close"
            && failed["status"] == "failed",
        "原失败close与完整前像或当前来源不符",
    )?;
    let number = failed["number"].as_u64().ok_or("原关闭编号无效")?;
    let root = run.join(format!("lifecycle-{number:04}"));
    check(
        !digest::linked_path(&root)?.contains(&true) && std::fs::read_dir(&root)?.next().is_none(),
        "关闭恢复只接受原失败目录完整保留且无停止收据或未知写入",
    )?;
    let (rustfs, redis) = generation::paths(run, generation)?;
    let windows = services["requests"]["redis"]["format_version"] == 3
        || services["runtime"]["format_version"] == 3;
    if !windows {
        return records(run, failed, services, closed, &rustfs, &redis, false);
    }
    check(
        !digest::linked_path(&redis)?.contains(&true),
        "Windows Redis 代次目录不能经过链接",
    )?;
    let protected = ProtectedInputs::open(WINDOWS_REDIS_FILES.iter().map(|name| redis.join(name)))?;
    protected.finish(records(
        run, failed, services, closed, &rustfs, &redis, true,
    ))
}

fn records(
    run: &Path,
    failed: &Value,
    services: &Value,
    closed: bool,
    rustfs: &Path,
    redis: &Path,
    windows: bool,
) -> Result<Value> {
    let runtime_path = redis.join("runtime.json");
    let request_path = redis.join("request.json");
    let exit_path = redis.join(if windows {
        "exited.json"
    } else {
        "linux-exited.json"
    });
    let runtime = read(&runtime_path)?;
    let request = read(&request_path)?;
    let exited = read(&exit_path)?;
    if windows {
        windows_exit(redis, &request, &runtime, &exited, services)?;
    } else {
        exact(&exited, &["identity", "returncode"])?;
        check(
            runtime == services["runtime"]
                && request == services["requests"]["redis"]
                && request["format_version"] == 2
                && runtime["request_sha256"] == digest::plan_hash(&request)?
                && exited["identity"] == runtime["linux_identity"]
                && exited["returncode"] == 0
                && runtime["boot_id"] == runtime["linux_identity"]["boot_id"],
            "关闭恢复缺少原请求、runtime或正常退出身份",
        )?;
    }
    let before = chrono::DateTime::parse_from_rfc3339(text(failed, "started_at")?)?;
    let after = chrono::DateTime::parse_from_rfc3339(text(failed, "finished_at")?)?;
    let modified = std::fs::metadata(&exit_path)?.modified()?;
    let modified_utc: chrono::DateTime<chrono::Utc> = modified.into();
    check(
        before <= modified_utc && modified_utc <= after && before < after,
        "Redis正常退出证据不在原失败close因果时间内",
    )?;
    let tree_path = rustfs.join("rustfs-tree.json");
    let tree = read(&tree_path)?;
    check(
        tree == services["tree"] && services["evidence"]["rustfs_tree"] == binding(&tree_path)?,
        "关闭恢复的原RustFS完整树变化",
    )?;
    if !closed {
        completed_records(run, rustfs, &tree)?;
    }
    let result = json!({"failed_attempt":failed["number"],"failed_controller":controller(run,failed)?,"failed_sources":failed["sources"],
        "request":binding(&request_path)?,"runtime":binding(&runtime_path)?,"exit":binding(&exit_path)?,
        "exit_modified_unix_ns":modified.duration_since(UNIX_EPOCH)?.as_nanos().to_string(),"rustfs_tree":binding(&tree_path)?,"redis_files":redis_files(redis,windows)?});
    if windows {
        check(
            std::fs::metadata(&exit_path)?.modified()? == modified,
            "Windows Redis 原退出收据时间在关闭恢复核验期间变化",
        )?;
    }
    Ok(result)
}

fn windows_exit(
    redis: &Path,
    request: &Value,
    runtime: &Value,
    exited: &Value,
    services: &Value,
) -> Result<()> {
    let process_path = redis.join("process.json");
    let process = read(&process_path)?;
    exact(&process, &["launch", "supervisor", "process"])?;
    exact(exited, &["process", "returncode", "job_empty"])?;
    common::validate_process_identity(&runtime["process"])?;
    check(
        request["format_version"] == 3
            && runtime["format_version"] == 3
            && request == &services["requests"]["redis"]
            && runtime == &services["runtime"]
            && runtime["request_sha256"] == digest::plan_hash(request)?
            && Path::new(text(runtime, "output")?) == redis
            && process["launch"] == runtime["launch"]
            && process["supervisor"] == runtime["supervisor"]
            && process["process"] == runtime["process"]
            && exited["process"] == runtime["process_receipt"]
            && common::binding(&process_path)? == runtime["process_receipt"]
            && exited["returncode"] == 0
            && exited["job_empty"] == true,
        "关闭恢复缺少 Windows Redis 原请求、运行代次或完整正常退出证明",
    )
}

fn completed_records(run: &Path, directory: &Path, tree: &Value) -> Result<()> {
    let operation = text(tree, "operation_id")?;
    let control = directory.join(format!("rustfs-tree-{operation}-control.json"));
    let result = directory.join(format!("rustfs-tree-{operation}-result.json"));
    if !control.try_exists()? && !result.try_exists()? {
        return Ok(());
    }
    if result.try_exists()? {
        let value = read(&result)?;
        exact(
            &value,
            &[
                "format_version",
                "kind",
                "operation_id",
                "process",
                "exit_code",
                "termination",
            ],
        )?;
        check(
            value["format_version"] == 1
                && value["kind"] == "full-stack-process-tree-result"
                && value["operation_id"] == tree["operation_id"]
                && value["process"] == tree["process"]
                && (value["exit_code"].is_i64() || value["exit_code"].is_u64())
                && ["natural", "normal", "forced"]
                    .iter()
                    .any(|mode| value["termination"] == *mode),
            "原RustFS完成结果不属于登记树",
        )?;
    }
    if control.try_exists()? {
        let request = read(&control)?;
        exact(
            &request,
            &[
                "format_version",
                "kind",
                "operation_id",
                "mode",
                "requested_at_ns",
            ],
        )?;
        check(
            request["format_version"] == 1
                && request["kind"] == "full-stack-process-tree-control"
                && request["operation_id"] == tree["operation_id"]
                && request["requested_at_ns"]
                    .as_u64()
                    .is_some_and(|time| time > 0)
                && ["normal", "crash"]
                    .iter()
                    .any(|mode| request["mode"] == *mode),
            "原RustFS控制请求不属于登记树",
        )?;
    }
    members(
        &json!({"state":"stopped","tree":tree,"completion":binding(&directory.join(format!("rustfs-members-{operation}-stopped.json")))?}),
        run,
    )
}

fn redis_files(directory: &Path, windows: bool) -> Result<Value> {
    let linux = [
        "request.json",
        "launch-request.json",
        "intent.json",
        "launcher.json",
        "linux-intent.json",
        "linux-process.json",
        "acknowledged.json",
        "accepted.json",
        "runtime.json",
        "linux-exited.json",
    ];
    let windows_logs = ["redis.stdout.log", "redis.stderr.log"];
    let linux_logs = [
        "redis.stdout.log",
        "redis.stderr.log",
        "wsl.stdout.log",
        "wsl.stderr.log",
    ];
    let (required, logs) = if windows {
        (WINDOWS_REDIS_FILES, windows_logs.as_slice())
    } else {
        (linux.as_slice(), linux_logs.as_slice())
    };
    let mut result = json!({});
    for &name in required {
        result[name] = binding(&directory.join(name))?;
    }
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("原Redis文件名不是UTF-8")?;
        check(
            entry.file_type()?.is_file()
                && !digest::linked_path(&entry.path())?.contains(&true)
                && (required.contains(&name) || logs.contains(&name)),
            "原Redis目录含未知写入、链接或子目录",
        )?;
    }
    Ok(result)
}

pub(crate) fn recovery_evidence(
    run: &Path,
    failed: &Value,
    generation: &Value,
    sources: &Value,
    services: &Value,
    descriptor: &Value,
) -> Result<Value> {
    let value = bound(run, descriptor)?;
    exact(
        &value,
        &[
            "format_version",
            "kind",
            "owner",
            "candidate",
            "redis_observation",
            "rustfs_tree",
            "rustfs_members",
            "elapsed_millis",
            "budget_seconds",
            "windows_port",
            "remote_writes",
            "resources_deleted",
        ],
    )?;
    let path = Path::new(text(descriptor, "path")?);
    let number = prefix::count(run, &value["owner"])? + 1;
    check(
        path == run.join(format!("lifecycle-{number:04}/close-recovery.json"))
            && value["format_version"] == 1
            && value["kind"] == "reference-fixture-close-recovery"
            && value["owner"] == sources["state_before"]
            && value["budget_seconds"] == 180
            && value["elapsed_millis"]
                .as_u64()
                .is_some_and(|elapsed| elapsed < 180_000)
            && value["remote_writes"] == 0
            && value["resources_deleted"] == false,
        "关闭恢复收据不属于本次owner、阶段或固定期限",
    )?;
    // 关闭后的原树允许增加真实控制及成员证明，原失败目录仍必须为空。
    let candidate = &value["candidate"];
    check(
        *candidate == self::candidate(run, failed, generation, sources, services, true)?,
        "关闭恢复原失败来源或退出证据变化",
    )?;
    let (rustfs, redis) = generation::paths(run, generation)?;
    let windows = services["requests"]["redis"]["format_version"] == 3
        || services["runtime"]["format_version"] == 3;
    let runtime = if windows {
        bound(run, &candidate["runtime"])?
    } else {
        read(&redis.join("runtime.json"))?
    };
    exact(&value["windows_port"], &["port", "bound_and_closed"])?;
    check(
        value["windows_port"]
            == json!({"port":services["requests"]["redis"]["port"],"bound_and_closed":true}),
        "关闭恢复缺少原Windows端口独占绑定事实",
    )?;
    check(
        value["rustfs_tree"] == services["tree"],
        "关闭恢复原树不一致",
    )?;
    let observed = &value["redis_observation"];
    if windows {
        exact(observed, &["state", "launcher_alive", "windows"])?;
        exact(&observed["windows"], &["identity", "alive"])?;
        check(
            runtime == services["runtime"]
                && observed["state"] == "stopped"
                && observed["launcher_alive"] == false
                && observed["windows"]["identity"] == runtime["process"]
                && observed["windows"]["alive"] == false,
            "关闭恢复没有原 Windows Redis 身份只读退出证明",
        )?;
    } else {
        linux_observation(observed, &runtime)?;
    }
    let completion = &value["rustfs_members"];
    members(
        &json!({"state":"stopped","tree":services["tree"],"completion":completion}),
        run,
    )?;
    check(
        candidate["rustfs_tree"] == binding(&rustfs.join("rustfs-tree.json"))?,
        "原RustFS树变化",
    )?;
    Ok(
        json!({"close_recovery":descriptor,"rustfs_tree":candidate["rustfs_tree"],"rustfs_members":completion}),
    )
}

fn linux_observation(observed: &Value, runtime: &Value) -> Result<()> {
    exact(observed, &["state", "launcher_alive", "linux"])?;
    exact(
        &observed["linux"],
        &["identity", "alive", "boot_id", "terminated"],
    )?;
    check(
        observed["state"] == "stopped"
            && observed["launcher_alive"] == false
            && observed["linux"]["identity"] == runtime["linux_identity"]
            && observed["linux"]["alive"] == false
            && observed["linux"]["boot_id"] == runtime["boot_id"]
            && observed["linux"]["terminated"] == false,
        "关闭恢复没有原身份只读退出证明",
    )
}
