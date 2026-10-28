use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::evidence::{binding, bound, check, exact, read, text};
use crate::Result;

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
    exact(
        evidence,
        &["rustfs_tree", "redis_runtime", "rustfs_monitor_ready"],
    )?;
    check(
        proof["format_version"] == 1
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
    validate_redis(
        &read(&run.join("redis/request.json"))?,
        &current,
        &item["sources"]["review"],
    )
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
