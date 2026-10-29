//! 失败 close 的新阶段恢复证据；原失败目录及停止收据缺失事实不可改写。
use super::*;
use std::time::UNIX_EPOCH;

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
    let runtime_path = redis.join("runtime.json");
    let request_path = redis.join("request.json");
    let exit_path = redis.join("linux-exited.json");
    let runtime = read(&runtime_path)?;
    let request = read(&request_path)?;
    let exited = read(&exit_path)?;
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
    for suffix in ["control", "result"].into_iter().filter(|_| !closed) {
        check(
            !rustfs
                .join(format!(
                    "rustfs-tree-{}-{suffix}.json",
                    text(&tree, "operation_id")?
                ))
                .try_exists()?,
            "原RustFS树存在未确认关闭请求或结果",
        )?;
    }
    Ok(
        json!({"failed_attempt":number,"failed_controller":controller(run,failed)?,"failed_sources":failed["sources"],
        "request":binding(&request_path)?,"runtime":binding(&runtime_path)?,"exit":binding(&exit_path)?,
        "exit_modified_unix_ns":modified.duration_since(UNIX_EPOCH)?.as_nanos().to_string(),"rustfs_tree":binding(&tree_path)?,"redis_files":redis_files(&redis)?}),
    )
}

fn redis_files(directory: &Path) -> Result<Value> {
    let required = [
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
    let logs = [
        "redis.stdout.log",
        "redis.stderr.log",
        "wsl.stdout.log",
        "wsl.stderr.log",
    ];
    let mut result = json!({});
    for name in required {
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
    let runtime = read(&redis.join("runtime.json"))?;
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
    )?;
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
