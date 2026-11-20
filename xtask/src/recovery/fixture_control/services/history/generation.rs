use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::evidence::{binding, bound, check, exact, read, text};
use crate::{
    Result,
    recovery::clone::native::{
        cache::windows_tool, digest, resources::common::validate_process_identity,
    },
};

pub(super) fn paths(run: &Path, generation: &Value) -> Result<(PathBuf, PathBuf)> {
    if generation["kind"] == "initial" {
        return Ok((run.join("rustfs"), run.join("redis")));
    }
    let result = bound(run, generation)?;
    let document = bound(run, &result["generation"])?;
    let rustfs = Path::new(text(&document["rustfs"]["request"], "path")?)
        .parent()
        .ok_or("RustFS 代次路径缺少父目录")?;
    let redis = Path::new(text(&document["redis"]["request"], "path")?)
        .parent()
        .ok_or("Redis 代次路径缺少父目录")?;
    check(
        rustfs.starts_with(run) && redis.starts_with(run),
        "服务代次越过运行目录",
    )?;
    Ok((rustfs.to_path_buf(), redis.to_path_buf()))
}

pub(super) fn external(proof: &Value, run: &Path, owner: &Value, generation: &Value) -> Result<()> {
    exact(
        proof,
        &[
            "format_version",
            "kind",
            "run",
            "owner",
            "observation",
            "normal_shutdown_proof",
            "remote_writes",
            "resources_deleted",
        ],
    )?;
    let observation = &proof["observation"];
    let evidence = &observation["evidence"];
    let (rustfs, redis) = paths(run, generation)?;
    use super::super::lifecycle::incident;
    if proof["format_version"] == incident::VERSION {
        check(
            proof["kind"] == "reference-fixture-external-termination"
                && super::evidence::path_is(&proof["run"], run)
                && &proof["owner"] == owner
                && proof["normal_shutdown_proof"].is_null()
                && proof["remote_writes"] == 0
                && proof["resources_deleted"] == false,
            "主机重启证据未绑定原服务账本或错误记录正常关闭",
        )?;
        return incident::verify_historical(run, &rustfs, &redis, observation);
    }
    let external_redis = proof["format_version"] == 2;
    let mut fields = vec!["rustfs_tree", "redis_runtime", "rustfs_monitor_ready"];
    if external_redis {
        fields.push("redis_external");
        exact(
            observation,
            &["status", "identities", "normal_shutdown_proof", "evidence"],
        )?;
    }
    exact(evidence, &fields)?;
    check(
        (proof["format_version"] == 1 || external_redis)
            && proof["kind"] == "reference-fixture-external-termination"
            && super::evidence::path_is(&proof["run"], run)
            && &proof["owner"] == owner
            && proof["normal_shutdown_proof"].is_null()
            && proof["remote_writes"] == 0
            && proof["resources_deleted"] == false
            && observation["status"] == "external-termination-unreconciled"
            && observation["normal_shutdown_proof"].is_null()
            && observation["identities"]
                == json!({"supervisor":"missing","monitor":"missing","process":"missing"})
            && evidence["rustfs_tree"] == binding(&rustfs.join("rustfs-tree.json"))?
            && evidence["redis_runtime"] == binding(&redis.join("runtime.json"))?,
        "夹具外部终止证据未证明全部身份消失或原运行来源",
    )?;
    let ready_path = Path::new(text(&evidence["rustfs_monitor_ready"], "path")?);
    check(
        ready_path.parent() == Some(rustfs.as_path())
            && ready_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("-ready.json"))
            && evidence["rustfs_monitor_ready"] == binding(ready_path)?,
        "夹具外部终止证据未绑定原成员监督启动归属",
    )?;
    if external_redis {
        validate_external_redis(evidence, run, &redis)?;
    }
    Ok(())
}

fn validate_external_redis(evidence: &Value, run: &Path, directory: &Path) -> Result<()> {
    let runtime = bound(run, &evidence["redis_runtime"])?;
    let request = read(&directory.join("request.json"))?;
    validate_process_identity(&runtime["supervisor"])?;
    validate_process_identity(&runtime["process"])?;
    check(
        runtime["format_version"] == 3
            && request["format_version"] == 3
            && runtime["redis"]["platform"] == "windows"
            && runtime["request_sha256"] == digest::plan_hash(&request)?
            && runtime["redis"]["port"] == request["port"]
            && request["port"]
                .as_u64()
                .is_some_and(|port| (1..=65535).contains(&port))
            && runtime["supervisor"]["pid"] != runtime["process"]["pid"]
            && evidence["redis_external"]
                == json!({
                "kind":"windows-redis-external-termination","runtime":evidence["redis_runtime"],
                "supervisor":runtime["supervisor"],"process":runtime["process"],"port":request["port"],
                "normal_job_exit_proof":null}),
        "夹具 Redis 外部终止证据未绑定原 Windows 代次或错误记录正常 Job 退出",
    )
}

pub(super) fn restart(
    proof: &Value,
    run: &Path,
    number: usize,
    item: &Value,
    predecessor: &Value,
    previous: &Value,
    controller: &Value,
) -> Result<()> {
    exact(
        proof,
        &[
            "format_version",
            "kind",
            "run",
            "attempt",
            "owner",
            "predecessor",
            "previous_generation",
            "controller",
            "services",
            "rustfs",
            "redis",
            "remote_writes",
            "resources_deleted",
        ],
    )?;
    let root = run.join(format!("lifecycle-{number:04}"));
    let rustfs = &proof["rustfs"];
    let redis = &proof["redis"];
    exact(rustfs, &["request", "storage"])?;
    exact(redis, &["request", "runtime"])?;
    check(
        proof["format_version"] == 1
            && proof["kind"] == "reference-fixture-service-generation"
            && super::evidence::path_is(&proof["run"], run)
            && proof["attempt"] == number
            && proof["owner"] == item["sources"]["state_before"]
            && &proof["predecessor"] == predecessor
            && &proof["previous_generation"] == previous
            && &proof["controller"] == controller
            && proof["services"] == json!({"redis":"running","rustfs":"running"})
            && proof["remote_writes"] == 0
            && proof["resources_deleted"] == false,
        "夹具服务重启代次未绑定先行状态及核对结果",
    )?;
    let rustfs_path = root.join("rustfs/request.json");
    let redis_path = root.join("redis/request.json");
    check(
        super::evidence::path_is(&rustfs["request"]["path"], &rustfs_path)
            && super::evidence::path_is(&redis["request"]["path"], &redis_path)
            && bound(run, &rustfs["request"])? == read(&run.join("rustfs/request.json"))?,
        "夹具服务重启请求未复用原物理资源或不属于本代次",
    )?;
    let current = bound(run, &redis["request"])?;
    let storage = &rustfs["storage"];
    exact(
        storage,
        &[
            "identity",
            "sha256",
            "process_receipt",
            "launch_receipt",
            "tree",
        ],
    )?;
    for key in ["process_receipt", "launch_receipt", "tree"] {
        check(
            Path::new(text(&storage[key], "path")?).parent() == Some(root.join("rustfs").as_path()),
            "夹具 RustFS 重启没有绑定新进程树及启动收据",
        )?;
        bound(run, &storage[key])?;
    }
    let runtime = &redis["runtime"];
    check(
        runtime.is_object()
            && super::evidence::path_is(&runtime["output"], &root.join("redis"))
            && read(&root.join("redis/runtime.json"))? == *runtime,
        "夹具 Redis 重启没有绑定新运行收据",
    )?;
    let original = read(&run.join("redis/request.json"))?;
    let review = &item["sources"]["review"];
    if original["format_version"] == 3 {
        check(
            runtime["format_version"] == 3
                && runtime["request_sha256"] == digest::plan_hash(&current)?,
            "Windows Redis 重启运行收据未绑定本次 v3 请求",
        )?;
        return validate_windows_redis(run, previous, &original, &current, review);
    }
    validate_redis(&original, &current, review)
}

pub(super) fn validate_windows_redis(
    run: &Path,
    previous: &Value,
    original: &Value,
    current: &Value,
    review: &Value,
) -> Result<()> {
    let tool = &original["tool"];
    windows_tool::validate(tool)?;
    check(
        current["format_version"] == 3
            && original.get("previous_boot_id").is_none()
            && current.get("previous_boot_id").is_none(),
        "Windows Redis 重启请求必须保持 v3 且不含 Linux boot 字段",
    )?;
    let runtime = previous_windows_runtime(run, previous, original)?;
    let process = &runtime["process"];
    exact(process, &["pid", "started", "executable"])?;
    let image = text(process, "executable")?.replace('\\', "/");
    let program = text(tool, "path")?.replace('\\', "/");
    let started = text(process, "started")?;
    let redis = &runtime["redis"];
    let run_id = text(redis, "run_id")?;
    check(
        process["pid"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .is_some_and(|pid| pid > 1)
            && !started.is_empty()
            && started.bytes().all(|byte| byte.is_ascii_digit())
            && started.parse::<u64>().is_ok()
            && image.eq_ignore_ascii_case(&program)
            && redis["platform"] == "windows"
            && redis["tool"] == *tool
            && redis["pid"] == process["pid"]
            && redis["started"] == process["started"]
            && run_id.len() == 40
            && run_id
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "Windows Redis 前代缺少原工具、内核进程或认证运行代次",
    )?;
    let mut expected = original.clone();
    expected["previous_identity"] = process.clone();
    expected["previous_run_id"] = json!(run_id);
    check(
        current == &expected,
        "Windows Redis 重启未复用原请求或未绑定精确前驱",
    )?;
    if !review.is_null() {
        let path = Path::new(text(review, "path")?);
        check(binding(path)? == *review, "Windows Redis 重启审阅发生变化")?;
        let document = read(path)?;
        let tools = &document["tools"];
        check(
            tools["redis_server"] == *tool
                && ["wsl", "native_tool", "redis_python"]
                    .iter()
                    .all(|key| tools.get(*key).is_none())
                && binding(path)? == *review,
            "Windows Redis 重启审阅未绑定原工具或混有 Linux 工具",
        )?;
    }
    Ok(())
}

fn previous_windows_runtime(run: &Path, previous: &Value, original: &Value) -> Result<Value> {
    let (directory, request_hash, registered) = if previous["kind"] == "initial" {
        let result = bound(run, &previous["redis"])?;
        let hash = digest::plan_hash(original)?;
        check(
            result["service"] == "redis" && result["request_sha256"] == hash,
            "Windows Redis 首代结果未绑定原请求",
        )?;
        (run.join("redis"), hash, result["runtime"].clone())
    } else {
        let result = bound(run, previous)?;
        let generation = bound(run, &result["generation"])?;
        let attempt = generation["attempt"].as_u64().ok_or("Redis 前代编号无效")?;
        let directory = run.join(format!("lifecycle-{attempt:04}")).join("redis");
        let redis = &generation["redis"];
        check(
            super::evidence::path_is(&redis["request"]["path"], &directory.join("request.json")),
            "Windows Redis 前代请求不属于其已登记目录",
        )?;
        let request = bound(run, &redis["request"])?;
        check(
            request["format_version"] == 3 && request["tool"] == original["tool"],
            "Windows Redis 前代请求平台或工具不同",
        )?;
        (
            directory,
            digest::plan_hash(&request)?,
            redis["runtime"].clone(),
        )
    };
    let runtime = bound(run, &binding(&directory.join("runtime.json"))?)?;
    check(
        runtime == registered
            && runtime["format_version"] == 3
            && runtime["request_sha256"] == request_hash
            && super::evidence::path_is(&runtime["output"], &directory),
        "Windows Redis 原前代运行收据与已登记请求或结果不同",
    )?;
    Ok(runtime)
}

fn validate_redis(original: &Value, current: &Value, review: &Value) -> Result<()> {
    let skipped = ["previous_identity", "previous_boot_id", "previous_run_id"];
    let select = |value: &Value| -> Result<Map<String, Value>> {
        Ok(value
            .as_object()
            .ok_or("Redis 服务请求必须为对象")?
            .iter()
            .filter(|(key, _)| !skipped.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect())
    };
    let mut expected = select(original)?;
    let stable = select(current)?;
    if !review.is_null() {
        let path = Path::new(text(review, "path")?);
        check(binding(path)? == *review, "夹具服务重启工具审阅缺失或变化")?;
        let document = read(path)?;
        let tools = &document["tools"];
        let projection = tool_projection(tools)?;
        for key in [
            "wsl",
            "python",
            "native_tool",
            "executable",
            "sha256",
            "distribution",
        ] {
            if expected.contains_key(key)
                && stable.get(key) != expected.get(key)
                && let Some(value) = projection.get(key)
            {
                expected.insert(key.to_owned(), value.clone());
            }
        }
    }
    check(
        stable == expected && skipped.iter().all(|key| !current[*key].is_null()),
        "夹具 Redis 重启未绑定前一运行代次、原物理配置与本次工具审阅",
    )
}

fn tool_projection(tools: &Value) -> Result<Map<String, Value>> {
    let mut values = Map::new();
    if tools["native_tool"].is_object() {
        crate::recovery::clone::native::cache::validate_native_tool(&tools["native_tool"])?;
        values.insert("native_tool".into(), tools["native_tool"].clone());
    }
    if tools["wsl"].is_object() {
        values.insert(
            "wsl".into(),
            json!({"path":tools["wsl"]["path"],"sha256":tools["wsl"]["sha256"]}),
        );
    }
    if tools["redis_python"].is_object() {
        values.insert("python".into(),json!({"path":tools["redis_python"]["path"],"executable":tools["redis_python"]["resolved_path"],"sha256":tools["redis_python"]["sha256"]}));
    }
    if tools["redis_server"].is_object() {
        for (key, source) in [
            ("executable", "resolved_path"),
            ("sha256", "sha256"),
            ("distribution", "distribution"),
        ] {
            values.insert(key.into(), tools["redis_server"][source].clone());
        }
    }
    Ok(values)
}

#[cfg(test)]
#[test]
fn windows_restart_rejects_drift_from_the_registered_predecessor_and_review() {
    use crate::recovery::clone::native::json::write_new;

    let base = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&base).unwrap();
    let fixture = tempfile::tempdir_in(base).unwrap();
    let run = fixture.path();
    let tool = json!({"platform":"windows","path":"D:/tools/redis-server.exe","sha256":"a".repeat(64),
        "version":"Redis server v=7.4.11 bits=64","dlls":{"cygwin1.dll":"b".repeat(64)}});
    let original = json!({"format_version":3,"tool":tool,"configuration":{"path":"D:/data/redis.conf"},
        "previous_identity":null,"previous_run_id":null,"port":16390});
    let review = run.join("review.json");
    write_new(&review, &json!({"tools":{"redis_server":tool}})).unwrap();
    let review = binding(&review).unwrap();
    let wrong_review = run.join("wrong-review.json");
    let mut changed_tool = tool.clone();
    changed_tool["sha256"] = json!("c".repeat(64));
    write_new(
        &wrong_review,
        &json!({"tools":{"redis_server":changed_tool}}),
    )
    .unwrap();
    let wrong_review = binding(&wrong_review).unwrap();

    let mut current = original.clone();
    for (number, pid, run_id) in [(1, 36784, "a".repeat(40)), (5, 36786, "b".repeat(40))] {
        let stage = run.join(format!("lifecycle-{number:04}"));
        let directory = if number == 1 {
            run.join("redis")
        } else {
            stage.join("redis")
        };
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("request.json");
        write_new(&path, &current).unwrap();
        let hash = digest::plan_hash(&current).unwrap();
        let process = json!({"pid":pid,"started":"134000000000000000",
            "executable":r"D:\tools\redis-server.exe"});
        let runtime = json!({"format_version":3,"request_sha256":hash,"output":directory,
            "process":process,"redis":{"platform":"windows","tool":tool,"pid":pid,
                "started":process["started"],"run_id":run_id}});
        let runtime_path = directory.join("runtime.json");
        write_new(&runtime_path, &runtime).unwrap();
        let result = run.join(format!("result-{number}.json"));
        let previous = if number == 1 {
            write_new(
                &result,
                &json!({"service":"redis","request_sha256":hash,"runtime":runtime}),
            )
            .unwrap();
            json!({"kind":"initial","redis":binding(&result).unwrap()})
        } else {
            let generation = stage.join("restart.json");
            write_new(
                &generation,
                &json!({"attempt":number,
                "redis":{"request":binding(&path).unwrap(),"runtime":runtime}}),
            )
            .unwrap();
            write_new(
                &result,
                &json!({"generation":binding(&generation).unwrap()}),
            )
            .unwrap();
            binding(&result).unwrap()
        };
        current["previous_identity"] = process;
        current["previous_run_id"] = json!(run_id);
        validate_windows_redis(run, &previous, &original, &current, &review).unwrap();
        for (key, value) in [
            (
                "previous_identity",
                json!({"pid":1,"started":"1","executable":r"D:\tools\redis-server.exe"}),
            ),
            ("previous_run_id", json!("f".repeat(40))),
            ("previous_boot_id", Value::Null),
            ("port", json!(16391)),
        ] {
            let mut changed = current.clone();
            changed[key] = value;
            assert!(validate_windows_redis(run, &previous, &original, &changed, &review).is_err());
        }
        assert!(
            validate_windows_redis(run, &previous, &original, &current, &wrong_review).is_err()
        );
        let mut changed_runtime = runtime;
        changed_runtime["redis"]["run_id"] = json!("f".repeat(40));
        std::fs::write(runtime_path, serde_json::to_vec(&changed_runtime).unwrap()).unwrap();
        assert!(validate_windows_redis(run, &previous, &original, &current, &review).is_err());
    }
}
