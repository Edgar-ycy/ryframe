//! 初始化只读观察的精确多重集，拒绝将未知文件吸收到续作基线。
use super::{
    super::{digest, json as json_io},
    evidence_model::*,
};
use crate::Result;
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/target_verify/resume_observations/tests.rs"]
mod tests;

pub(super) fn receipt(backend: &Path, target: &Path, name: &str) -> Result<Value> {
    let mut value = read(backend, &target.join(name))?;
    json_io::exact(
        &value,
        &["command", "returncode", "error_type", "stdout", "stderr"],
        "只读命令收据",
    )?;
    let command = json_io::array(&value["command"], "只读命令")?;
    check(
        !command.is_empty()
            && command
                .iter()
                .all(|item| item.as_str().is_some_and(|value| !value.is_empty()))
            && value["returncode"].as_i64() == Some(0)
            && value["error_type"].is_null()
            && value["stdout"].is_string()
            && value["stderr"] == "",
        "迁移后继只读观察命令收据无效",
    )?;
    value["stdout"] = json!(json_io::text(&value["stdout"], "只读输出")?.replace("\r\n", "\n"));
    Ok(value)
}

pub(super) fn stage(names: &BTreeSet<String>, stage: &str) -> Result<Vec<String>> {
    let pattern = Regex::new(&format!(
        r"^{}-[a-f0-9]{{32}}\.command\.json$",
        regex::escape(stage)
    ))?;
    Ok(names
        .iter()
        .filter(|name| pattern.is_match(name))
        .cloned()
        .collect())
}

pub(super) fn file_map(files: &Value) -> Result<BTreeMap<String, Value>> {
    let mut map = BTreeMap::new();
    for item in json_io::array(files, "目标文件树")? {
        let file = item["type"] == "file";
        check(file || item["type"] == "directory", "目标文件快照条目无效")?;
        let fields = if file {
            &["path", "type", "bytes", "sha256"][..]
        } else {
            &["path", "type"][..]
        };
        json_io::exact(item, fields, "prepare 文件")?;
        let name = json_io::text(&item["path"], "prepare 文件名")?;
        check(
            !name.is_empty()
                && !name.contains(['\\', ':'])
                && !name.starts_with('/')
                && !name.split('/').any(|part| ["", ".", ".."].contains(&part))
                && !map.contains_key(name)
                && (!file
                    || (item["bytes"].as_u64().is_some()
                        && super::super::input::digest_shape(&item["sha256"]).is_ok())),
            "fresh 目标文件快照路径重复、越界或字段无效",
        )?;
        map.insert(name.to_owned(), item.clone());
    }
    Ok(map)
}

fn object_receipt(
    backend: &Path,
    target: &Path,
    name: &str,
    operation: &str,
    request: &Value,
) -> Result<Value> {
    let value = read(backend, &target.join(name))?;
    json_io::exact(
        &value,
        &[
            "transport",
            "operation",
            "bucket",
            "returncode",
            "error_type",
            "response",
            "key",
            "destination",
            "remote_writes",
            "scope_id",
            "endpoint",
            "configuration_sha256",
            "prefix",
            "continuation",
        ],
        "原生对象收据",
    )?;
    let scope = json_io::text(&request["target"]["scope_id"], "目标 scope")?;
    let prefix = if operation == "list-objects-v2" {
        json!(format!("{scope}/"))
    } else {
        Value::Null
    };
    check(
        value["transport"] == "rust-s3-sigv4"
            && value["operation"] == operation
            && value["bucket"]
                .as_str()
                .is_some_and(|bucket| BUCKETS.contains(&bucket))
            && value["returncode"].as_i64() == Some(0)
            && value["error_type"].is_null()
            && value["response"].is_object()
            && value["scope_id"] == scope
            && value["endpoint"] == request["target"]["s3"]["endpoint"]
            && value["configuration_sha256"] == digest::plan_hash(&request["target"]["s3"])?
            && value["prefix"] == prefix
            && value["continuation"].is_null()
            && value["remote_writes"].as_u64() == Some(0),
        "fresh 初始化原生对象只读观察收据无效",
    )?;
    Ok(value)
}

pub(super) fn objects(
    backend: &Path,
    target: &Path,
    names: &BTreeSet<String>,
    request: &Value,
    stage_name: &str,
) -> Result<Vec<Value>> {
    let operation = match stage_name {
        "bucket-readiness" => "head-bucket",
        "bucket-versioning" => "get-bucket-versioning",
        "scoped-list" => "list-objects-v2",
        _ => return Err("未知对象观察阶段".into()),
    };
    stage(names, stage_name)?
        .iter()
        .map(|name| {
            let value = object_receipt(backend, target, name, operation, request)?;
            check(
                value["key"].is_null() && value["destination"].is_null(),
                "fresh prepare 原生对象只读观察收据无效",
            )?;
            Ok(value)
        })
        .collect()
}

pub(super) fn process_identity(value: &str) -> Result<(String, String)> {
    let (head, fields) = value
        .trim()
        .rsplit_once(')')
        .ok_or("Redis 内核身份输出无效")?;
    let parts = fields.split_whitespace().collect::<Vec<_>>();
    check(
        !head.is_empty()
            && parts.len() >= 20
            && parts[19].bytes().all(|byte| byte.is_ascii_digit()),
        "Redis 内核身份输出无效",
    )?;
    Ok((format!("{head})"), parts[19].to_owned()))
}

fn signature(value: &Value) -> Result<String> {
    let command = json_io::array(&value["command"], "Redis 命令")?;
    let last = command
        .last()
        .and_then(Value::as_str)
        .ok_or("Redis 命令为空")?;
    let stdout = if last.contains("/proc/") && last.ends_with("/stat") {
        json!(process_identity(json_io::text(
            &value["stdout"],
            "Redis 输出"
        )?)?)
    } else {
        value["stdout"].clone()
    };
    digest::plan_hash(&json!([value["command"], stdout]))
}

fn counts(values: impl IntoIterator<Item = String>) -> BTreeMap<String, usize> {
    let mut result = BTreeMap::new();
    for value in values {
        *result.entry(value).or_default() += 1;
    }
    result
}

pub(super) fn redis(
    backend: &Path,
    target: &Path,
    baseline: &BTreeSet<String>,
    current: &BTreeSet<String>,
    rounds: usize,
) -> Result<()> {
    let baseline = stage(baseline, "redis-kernel")?
        .iter()
        .map(|name| receipt(backend, target, name))
        .collect::<Result<Vec<_>>>()?;
    let current = stage(current, "redis-kernel")?
        .iter()
        .map(|name| receipt(backend, target, name))
        .collect::<Result<Vec<_>>>()?;
    let signatures = counts(baseline.iter().map(signature).collect::<Result<Vec<_>>>()?);
    check(
        baseline.len() == 10
            && signatures.len() == 5
            && signatures.values().all(|count| *count == 2),
        "fresh prepare Redis 内核观察不是五项两轮",
    )?;
    let actual = counts(current.iter().map(signature).collect::<Result<Vec<_>>>()?);
    check(
        actual == signatures.into_keys().map(|key| (key, rounds)).collect(),
        "fresh 初始化 Redis 工具、配置或进程身份发生变化",
    )
}

pub(super) struct Delta<'a> {
    pub(super) required: &'a BTreeSet<String>,
    pub(super) rounds: usize,
    pub(super) initialized: Option<&'a Value>,
}

pub(super) fn delta(
    backend: &Path,
    target: &Path,
    current: &Value,
    baseline: &Value,
    request: &Value,
    delta: Delta<'_>,
) -> Result<()> {
    let Delta {
        required,
        rounds,
        initialized,
    } = delta;
    let baseline = file_map(baseline)?;
    let current = file_map(current)?;
    check(
        baseline
            .iter()
            .all(|(name, value)| current.get(name) == Some(value)),
        "fresh 目标 prepare 基线文件缺失或摘要变化",
    )?;
    check(
        current.get("copy-target.guard")
            == Some(
                &json!({"path":"copy-target.guard","type":"file","bytes":1,"sha256":sha(&[0])}),
            ),
        "fresh 初始化控制互斥文件无效",
    )?;
    check(
        required.iter().all(|name| current.contains_key(name)),
        "fresh 初始化缺少固定阶段文件",
    )?;
    let before = baseline.keys().cloned().collect::<BTreeSet<_>>();
    let mut remaining = current
        .keys()
        .filter(|name| {
            !baseline.contains_key(*name)
                && !required.contains(*name)
                && name.as_str() != "copy-target.guard"
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    for stage_name in ["bucket-readiness", "bucket-versioning", "scoped-list"] {
        object_multiset(
            backend,
            target,
            &before,
            &remaining,
            request,
            stage_name,
            initialized.is_some(),
        )?;
        for name in stage(&remaining, stage_name)? {
            remaining.remove(&name);
        }
    }
    if let Some(objects) = initialized {
        owner_reads(backend, target, &mut remaining, request, objects)?;
    }
    mysql(backend, target, &before, &mut remaining, request)?;
    redis(backend, target, &before, &remaining, rounds)?;
    for name in stage(&remaining, "redis-kernel")? {
        remaining.remove(&name);
    }
    let pattern = Regex::new(r"^storage-layout-[a-f0-9]{32}\.json$")?;
    let layouts = before
        .iter()
        .filter(|name| pattern.is_match(name))
        .map(|name| read(backend, &target.join(name)))
        .collect::<Result<Vec<_>>>()?;
    let names = remaining
        .iter()
        .filter(|name| pattern.is_match(name))
        .cloned()
        .collect::<Vec<_>>();
    check(
        layouts.len() == 2 && layouts[0] == layouts[1] && names.len() == rounds,
        "fresh 初始化存储代次观察收据无效",
    )?;
    for name in names {
        check(
            read(backend, &target.join(&name))? == layouts[0],
            "fresh 初始化存储代次观察收据无效",
        )?;
        remaining.remove(&name);
    }
    check(
        remaining.is_empty(),
        "fresh 初始化包含未登记的普通文件或目录",
    )
}

pub(super) fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn object_multiset(
    backend: &Path,
    target: &Path,
    before: &BTreeSet<String>,
    remaining: &BTreeSet<String>,
    request: &Value,
    stage_name: &str,
    initialized: bool,
) -> Result<()> {
    let baseline = objects(backend, target, before, request, stage_name)?;
    let current = objects(backend, target, remaining, request, stage_name)?;
    let repetitions = 3 + usize::from(initialized);
    let mut seen = BTreeSet::new();
    check(baseline.len() == 5, "fresh 初始化对象基线未覆盖五桶")?;
    for value in baseline {
        let bucket = json_io::text(&value["bucket"], "对象桶")?;
        check(
            seen.insert(bucket.to_owned()),
            "fresh 初始化对象基线未覆盖五桶",
        )?;
        let outputs = current
            .iter()
            .filter(|item| item["bucket"] == bucket)
            .collect::<Vec<_>>();
        check(
            outputs.len() == repetitions,
            "fresh 初始化对象观察命令不是逐桶精确多重集",
        )?;
        if initialized && stage_name == "scoped-list" {
            let final_values = outputs
                .iter()
                .filter(|item| item["response"] != value["response"])
                .collect::<Vec<_>>();
            check(
                final_values.len() == 1,
                "fresh 初始化对象观察命令不是逐桶精确多重集",
            )?;
            let page = &final_values[0]["response"];
            check(
                page["IsTruncated"] == false
                    && page["Contents"]
                        .as_array()
                        .is_some_and(|items| items.len() == 1)
                    && page["Contents"][0]["Key"]
                        == format!(
                            "{}/.ryframe-owner",
                            json_io::text(&request["target"]["scope_id"], "scope")?
                        ),
                "fresh 初始化对象观察命令不是逐桶精确多重集",
            )?;
        } else {
            check(
                outputs
                    .iter()
                    .all(|item| item["response"] == value["response"]),
                "fresh 初始化对象观察命令不是逐桶精确多重集",
            )?;
        }
    }
    Ok(())
}

fn mysql(
    backend: &Path,
    target: &Path,
    before: &BTreeSet<String>,
    remaining: &mut BTreeSet<String>,
    request: &Value,
) -> Result<()> {
    for db in json_io::array(&request["target"]["databases"], "目标数据库")? {
        let stage_name = format!("mysql-{}", json_io::text(&db["key"], "数据库键")?);
        let baseline = stage(before, &stage_name)?;
        let names = stage(remaining, &stage_name)?;
        check(
            baseline.len() == 1 && names.len() == 9,
            "fresh 初始化 MySQL 观察收据数量无效",
        )?;
        let original = receipt(backend, target, &baseline[0])?;
        let identity = format!("{}\n", json_io::text(&db["server_uuid"], "服务器 UUID")?);
        let exists = format!(
            "{identity}{}\n",
            json_io::text(&db["database"], "数据库名")?
        );
        check(
            original["stdout"] == identity,
            "fresh 初始化 MySQL 基线无效",
        )?;
        let mut outputs = Vec::new();
        for name in names {
            let value = receipt(backend, target, &name)?;
            check(
                value["command"] == original["command"],
                "fresh 初始化 MySQL 观察命令无效",
            )?;
            outputs.push(json_io::text(&value["stdout"], "MySQL 输出")?.to_owned());
            remaining.remove(&name);
        }
        check(
            counts(outputs) == BTreeMap::from([(identity, 2), (String::new(), 4), (exists, 3)]),
            "fresh 初始化 MySQL 前后像输出多重集无效",
        )?;
    }
    Ok(())
}

fn owner_reads(
    backend: &Path,
    target: &Path,
    remaining: &mut BTreeSet<String>,
    request: &Value,
    objects: &Value,
) -> Result<()> {
    let names = stage(remaining, "owner-read")?;
    check(
        names.len() == 5,
        "fresh 初始化对象 owner 证据未完整覆盖五桶",
    )?;
    json_io::exact(objects, &BUCKETS, "对象 owner 集合")?;
    let scope = json_io::text(&request["target"]["scope_id"], "scope")?;
    let mut seen = BTreeSet::new();
    for name in names {
        let value = object_receipt(backend, target, &name, "get-object", request)?;
        let bucket = json_io::text(&value["bucket"], "对象桶")?;
        let body = json_io::text(&value["destination"], "对象落盘文件")?;
        check(
            Regex::new(&format!(
                r"^owner-{}-[a-f0-9]{{32}}\.bin$",
                regex::escape(bucket)
            ))?
            .is_match(body)
                && seen.insert(bucket.to_owned()),
            "fresh 初始化对象 owner 命令无效",
        )?;
        let owner = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
        let key = format!("{scope}/.ryframe-owner");
        crate::local_test_path::validate_local_test_path(
            &target.join(body),
            backend,
            crate::local_test_path::LocalTestPathKind::ExistingFile,
        )?;
        check(
            value["key"] == key
                && std::fs::read(target.join(body))? == owner.as_bytes()
                && objects[bucket]
                    == json!({"keys":[key],"owner":{"bytes":owner.len(),"sha256":sha(owner.as_bytes())}}),
            "fresh 初始化对象 owner 命令、字节或结果无效",
        )?;
        remaining.remove(&name);
        remaining.remove(body);
    }
    Ok(())
}
