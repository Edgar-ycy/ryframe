//! 重读登记快照并验证 prepare 的所有观察；不信任调用方提供的文件清单。
use super::{
    super::{digest, json as json_io},
    evidence_model::*,
    resume_observations as observations,
};
use crate::Result;
use regex::Regex;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

pub(super) fn prepared(
    backend: &Path,
    target: &Path,
    evidence: &Value,
    descriptor: &Value,
) -> Result<Value> {
    json_io::exact(
        evidence,
        &["descriptor", "registration", "predecessor"],
        "prepare 文件绑定",
    )?;
    let path = bound(backend, &evidence["descriptor"])?;
    let snapshot = read(backend, &path)?;
    json_io::exact(
        &snapshot,
        &[
            "format_version",
            "kind",
            "registration",
            "predecessor",
            "files",
        ],
        "prepare 文件快照",
    )?;
    let registration_path = bound(backend, &evidence["registration"])?;
    let registration = read(backend, &registration_path)?;
    json_io::exact(
        &registration,
        &[
            "format_version",
            "kind",
            "request",
            "environment",
            "storage_run",
            "target_directory",
        ],
        "目标登记",
    )?;
    check(
        path.file_name()
            .is_some_and(|name| name == "prepared-files.json")
            && registration_path == path.with_file_name("registration.json")
            && path.parent() == target.parent()
            && snapshot["format_version"] == 1
            && snapshot["kind"] == "devex-clone-fresh-target-prepared-files"
            && snapshot["registration"] == evidence["registration"]
            && snapshot["predecessor"].is_null()
            && evidence["predecessor"].is_null()
            && registration["format_version"] == 1
            && registration["kind"] == "devex-clone-fresh-target-registration"
            && registration["request"] == *descriptor
            && Path::new(json_io::text(
                &registration["target_directory"],
                "目标目录",
            )?) == target,
        "fresh 目标 prepare 文件快照、registration 或 predecessor 绑定无效",
    )?;
    let request = read(backend, &bound(backend, descriptor)?)?;
    validate(backend, target, &snapshot["files"], descriptor, &request)?;
    check(
        binding(backend, &path)? == evidence["descriptor"]
            && binding(backend, &registration_path)? == evidence["registration"],
        "fresh 目标 prepare 文件快照或 registration 在读取期间变化",
    )?;
    Ok(snapshot["files"].clone())
}

fn document(backend: &Path, prepared: &Value, descriptor: &Value, request: &Value) -> Result<()> {
    json_io::exact(prepared, super::resume_model::PREPARE, "prepare")?;
    let generation = &prepared["generation"];
    json_io::exact(
        generation,
        &[
            "execution",
            "maintenance",
            "physical",
            "configuration",
            "configuration_sha256",
            "seed_credentials_sha256",
            "request_sha256",
            "review_sha256",
            "selected",
            "operator_declared_controlled_generation_only",
            "external_writers_discovered",
            "storage",
        ],
        "prepare generation",
    )?;
    let maintenance = read(backend, &bound(backend, &request["maintenance_build"])?)?;
    let selected = super::resume_model::selected(backend, request)?;
    let intents = json_io::array(&request["target"]["databases"], "目标数据库")?
        .iter()
        .map(
            |db| json!({"key":db["key"],"server_uuid":db["server_uuid"],"database":db["database"]}),
        )
        .collect::<Vec<_>>();
    let databases = json_io::array(&request["target"]["databases"],"目标数据库")?.iter().map(|db|json!({"server_uuid":db["server_uuid"],"database":db["database"],"exists":false,"empty_checked":false})).collect::<Vec<_>>();
    let objects = BUCKETS
        .into_iter()
        .map(|bucket| (bucket.to_owned(), json!({"keys":[]})))
        .collect::<serde_json::Map<_, _>>();
    check(
        prepared["format_version"] == 1
            && prepared["status"] == "fresh_creation_prepared"
            && timestamp(&prepared["prepared_at"]).is_ok()
            && prepared["request"] == *descriptor
            && prepared["request_sha256"] == digest::plan_hash(request)?
            && prepared["fresh_target_initialized"] == false
            && prepared["controlled_generation_only"] == true
            && prepared["external_writers_discovered"] == false
            && prepared["create_intents"] == json!(intents)
            && prepared["absence"]
                == json!({"databases":databases,"objects":objects,"redis":{"keys":[],"owner":null,"sentinel":null}})
            && generation["maintenance"] == maintenance
            && generation["configuration_sha256"] == request["configuration_sha256"]
            && generation["request_sha256"] == digest::plan_hash(request)?
            && generation["review_sha256"] == request["review"]["sha256"]
            && generation["selected"] == selected
            && generation["storage"] == request["storage"]
            && generation["operator_declared_controlled_generation_only"] == true
            && generation["external_writers_discovered"] == false
            && ["execution", "physical", "configuration"]
                .iter()
                .all(|key| generation[*key].is_object()),
        "fresh prepare 声明、来源或空资源前像无效",
    )
}

fn validate(
    backend: &Path,
    target: &Path,
    files: &Value,
    descriptor: &Value,
    request: &Value,
) -> Result<()> {
    let values = bound_files(backend, target, files)?;
    let names = values.keys().cloned().collect::<BTreeSet<_>>();
    let mut consumed = BTreeSet::from(["request.json".to_owned(), "prepare.json".to_owned()]);
    check(
        consumed.is_subset(&names) && read(backend, &target.join("request.json"))? == *request,
        "fresh prepare 快照缺少固定请求或结果",
    )?;
    document(
        backend,
        &read(backend, &target.join("prepare.json"))?,
        descriptor,
        request,
    )?;
    for db in json_io::array(&request["target"]["databases"], "目标数据库")? {
        let paths = observations::stage(
            &names,
            &format!("mysql-{}", json_io::text(&db["key"], "数据库键")?),
        )?;
        check(paths.len() == 1, "fresh prepare MySQL 观察数量无效")?;
        let receipt = observations::receipt(backend, target, &paths[0])?;
        check(
            receipt["stdout"] == format!("{}\n", json_io::text(&db["server_uuid"], "服务器 UUID")?),
            "fresh prepare MySQL 缺失观察不属于固定目标",
        )?;
        consumed.extend(paths);
    }
    observations::redis(backend, target, &names, &names, 2)?;
    let redis_paths = observations::stage(&names, "redis-kernel")?;
    let pid = request["storage"]["redis"]["pid"]
        .as_u64()
        .ok_or("Redis PID 无效")?;
    let mut stat_count = 0;
    for name in &redis_paths {
        let value = observations::receipt(backend, target, name)?;
        let command = json_io::array(&value["command"], "Redis 命令")?;
        if command.ends_with(&[json!("/usr/bin/cat"), json!(format!("/proc/{pid}/stat"))]) {
            stat_count += 1;
            let identity =
                observations::process_identity(json_io::text(&value["stdout"], "Redis 输出")?)?;
            check(
                identity.0.starts_with(&format!("{pid} (")),
                "fresh prepare Redis /proc stat 观察不属于固定进程",
            )?;
        }
    }
    check(
        stat_count == 2,
        "fresh prepare Redis /proc stat 观察不属于固定进程",
    )?;
    consumed.extend(redis_paths);
    let pattern = Regex::new(r"^storage-layout-[a-f0-9]{32}\.json$")?;
    let layouts = names
        .iter()
        .filter(|name| pattern.is_match(name))
        .collect::<Vec<_>>();
    check(layouts.len() == 2, "fresh prepare 存储目录观察数量无效")?;
    check(
        read(backend, &target.join(layouts[0]))? == read(backend, &target.join(layouts[1]))?,
        "fresh prepare 存储目录两轮观察不一致",
    )?;
    consumed.extend(layouts.into_iter().cloned());
    object_baseline(backend, target, &names, request, &mut consumed)?;
    if names.contains("failure.json")
        || names.iter().any(|name| name.starts_with("resume-prepare-"))
    {
        let records = super::evidence_prepare::records(backend, target, descriptor)?;
        check(
            records["pending"] == json!([])
                && json_io::object(&records["terminals"], "prepare 终态")?
                    .values()
                    .any(|item| item[0] == "confirmed"),
            "fresh prepare 失败续作谱系没有完整确认",
        )?;
        consumed.insert("failure.json".into());
        for value in json_io::object(&records["intents"], "prepare intent")?.values() {
            consumed.insert(super::resume_model::descriptor_name(value)?);
        }
        for value in json_io::object(&records["terminals"], "prepare 终态")?.values() {
            consumed.insert(super::resume_model::descriptor_name(&value[2])?);
        }
    }
    check(names == consumed, "fresh prepare 快照包含未声明的文件")
}

fn object_baseline(
    backend: &Path,
    target: &Path,
    names: &BTreeSet<String>,
    request: &Value,
    consumed: &mut BTreeSet<String>,
) -> Result<()> {
    for stage in ["bucket-readiness", "bucket-versioning", "scoped-list"] {
        let receipts = observations::objects(backend, target, names, request, stage)?;
        let buckets = receipts
            .iter()
            .map(|item| json_io::text(&item["bucket"], "对象桶").map(str::to_owned))
            .collect::<Result<BTreeSet<_>>>()?;
        check(
            receipts.len() == 5 && buckets == BUCKETS.map(str::to_owned).into(),
            "fresh prepare 对象观察未完整覆盖五桶",
        )?;
        for value in receipts {
            if stage == "bucket-versioning" {
                check(
                    value["response"]["Status"].is_null(),
                    "fresh prepare 对象桶启用了版本",
                )?;
            }
            if stage == "scoped-list" {
                check(
                    value["response"]["IsTruncated"] == false
                        && value["response"]
                            .get("Contents")
                            .is_none_or(|contents| contents == &json!([])),
                    "fresh prepare 对象前缀并非明确空范围",
                )?;
            }
        }
        consumed.extend(observations::stage(names, stage)?);
    }
    Ok(())
}

fn bound_files(
    backend: &Path,
    target: &Path,
    files: &Value,
) -> Result<std::collections::BTreeMap<String, Value>> {
    let values = observations::file_map(files)?;
    for (name, value) in &values {
        check(
            value["type"] == "file" && !name.contains('/'),
            "fresh prepare 快照包含目录、越界路径或变化文件",
        )?;
        check(
            *value == {
                let actual = binding(backend, &target.join(name))?;
                json!({"path":name,"type":"file","bytes":actual["bytes"],"sha256":actual["sha256"]})
            },
            "fresh prepare 快照包含目录、越界路径或变化文件",
        )?;
    }
    Ok(values)
}
