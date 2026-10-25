//! 迁移续作共享模型；只读取已绑定的本地证据。
use super::{
    super::{digest, json as json_io},
    evidence_model::*,
};
use crate::Result;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

pub(super) const PREPARE: &[&str] = &[
    "format_version",
    "status",
    "prepared_at",
    "request",
    "request_sha256",
    "generation",
    "absence",
    "fresh_target_initialized",
    "controlled_generation_only",
    "external_writers_discovered",
    "create_intents",
];
pub(super) const INITIALIZED: &[&str] = &[
    "format_version",
    "status",
    "id",
    "scope_id",
    "completed_at",
    "generation",
    "prepare_sha256",
    "reset",
    "inventory",
    "objects",
    "redis",
    "history",
    "controlled_generation_never_started",
    "external_writers_discovered",
    "target_ready",
    "clone_verified",
    "restore_qualified",
];
pub(super) const INTENT: &[&str] = &[
    "format_version",
    "kind",
    "attempt",
    "at",
    "request",
    "failure",
    "prepare",
    "initialize_started",
    "reset",
    "creation",
    "migration_prefix",
    "prepared_files",
    "files_before",
    "next_operation",
    "protected_binaries",
    "remote_write_operations_before_resume",
];
pub(super) const STARTED: &[&str] = &[
    "format_version",
    "kind",
    "attempt",
    "at",
    "intent",
    "current_generation_sha256",
    "storage",
    "storage_runtime",
    "cache_runtime",
    "resources_before",
];
pub(super) const OP_INTENT: &[&str] = &[
    "format_version",
    "kind",
    "attempt",
    "index",
    "at",
    "resume",
    "failure",
    "operation",
];
pub(super) const OP_CONFIRMED: &[&str] = &[
    "format_version",
    "kind",
    "attempt",
    "index",
    "at",
    "intent",
    "receipt",
    "write",
];
pub(super) const FAILURE: &[&str] = &[
    "format_version",
    "kind",
    "attempt",
    "at",
    "intent",
    "source_failure",
    "prepared_files",
    "started",
    "error_type",
    "confirmed_operations",
    "remote_write_operations_confirmed",
    "unknown_write_operation",
    "active_operation",
];

pub(super) fn selected(backend: &Path, request: &Value) -> Result<Value> {
    let descriptor = &request["review"];
    json_io::exact(
        descriptor,
        &["path", "bytes", "sha256", "canonical_sha256"],
        "审阅绑定",
    )?;
    let mut binding = descriptor.clone();
    binding
        .as_object_mut()
        .ok_or("审阅绑定无效")?
        .remove("canonical_sha256");
    let review = read(backend, &bound(backend, &binding)?)?;
    check(
        digest::plan_hash(&review)? == descriptor["canonical_sha256"],
        "目标审阅规范摘要不同",
    )?;
    super::input::request_review::validate(&review)?;
    let side = json_io::text(&request["side"], "目标单侧")?;
    let selected = review["scopes"].get(side).ok_or("目标审阅单侧缺失")?;
    check(
        selected["scope_id"] == request["target"]["scope_id"],
        "目标审阅 scope 不同",
    )?;
    Ok(selected.clone())
}

pub(super) fn binaries(backend: &Path, request: &Value) -> Result<Value> {
    selected(backend, request)?;
    let mut review_descriptor = request["review"].clone();
    review_descriptor
        .as_object_mut()
        .ok_or("审阅绑定无效")?
        .remove("canonical_sha256");
    let review = read(backend, &bound(backend, &review_descriptor)?)?;
    let build = read(backend, &bound(backend, &request["maintenance_build"])?)?;
    json_io::exact(
        &build["artifacts"],
        &["reset", "migrate", "tenant-data"],
        "维护构建二进制",
    )?;
    check(
        build["kind"] == "devex-clone-tool-build",
        "fresh 目标维护构建缺少完整二进制",
    )?;
    let mut bindings = json_io::object(&request["tools"], "目标工具")?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let build_path = bound(backend, &request["maintenance_build"])?;
    let bytes = std::fs::read(&build_path)?;
    check(
        request["maintenance_build"]["bytes"].as_u64() == Some(bytes.len() as u64)
            && request["maintenance_build"]["sha256"] == super::resume_observations::sha(&bytes),
        "维护构建收据在读取期间变化",
    )?;
    for artifact in artifact_bindings(&bytes)? {
        let path = json_io::text(&artifact["executable"], "维护程序")?;
        bindings.push(json!({"path":path,"sha256":artifact["sha256"]}));
    }
    let rustfs = json!({"path":request["storage"]["rustfs"]["identity"]["executable"],"sha256":request["storage"]["rustfs"]["sha256"]});
    check(
        rustfs == review["tools"]["rustfs"]
            && request["storage"]["redis"]["wsl"] == review["tools"]["wsl"],
        "fresh 目标存储二进制不属于审阅版本",
    )?;
    bindings.extend([rustfs, request["storage"]["redis"]["wsl"].clone()]);
    Ok(json!(bindings))
}

pub(super) fn owned(backend: &Path, request: &Value, value: &Value, markers: bool) -> Result<()> {
    json_io::exact(value, &["databases", "objects", "redis"], "迁移续作前像")?;
    let databases = json_io::array(&request["target"]["databases"], "目标数据库")?.iter().map(|db| json!({"server_uuid":db["server_uuid"],"database":db["database"],"exists":true,"empty_checked":false})).collect::<Vec<_>>();
    check(
        value["databases"] == json!(databases),
        "迁移续作的数据库或对象前像不属于固定 fresh 目标",
    )?;
    json_io::exact(&value["objects"], &BUCKETS, "迁移续作对象")?;
    let scope = json_io::text(&request["target"]["scope_id"], "目标 scope")?;
    for bucket in BUCKETS {
        let owner = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
        let expected = json!({"keys":[format!("{scope}/.ryframe-owner")],"owner":{"bytes":owner.len(),"sha256":super::resume_observations::sha(owner.as_bytes())}});
        check(
            value["objects"][bucket] == expected,
            "迁移续作的对象 owner 前像无效",
        )?;
    }
    let selected = selected(backend, request)?;
    let redis = &selected["redis"];
    let expected = if markers {
        json!({"keys":[redis["ownership_key"]],"owner":redis["ownership_value"],"sentinel":request["reset"]["sentinel_value"]})
    } else {
        json!({"keys":[],"owner":null,"sentinel":null})
    };
    check(
        value["redis"] == expected,
        "迁移续作的 Redis owner 前像无效",
    )
}

pub(super) fn operation(value: &Value) -> Value {
    json!({"id":value["id"],"command":value["command"],"write":value["write"]})
}

pub(super) fn failure(value: &Value) -> Result<bool> {
    json_io::exact(value, super::evidence_prepare::FAILURE, "迁移原失败")?;
    Ok(value["status"] == "needs_reconciliation"
        && timestamp(&value["at"]).is_ok()
        && value["stage"] == "create_and_reset"
        && value["error_type"] == "FileNotFoundError"
        && [
            "automatic_retry",
            "automatic_resource_cleanup",
            "fresh_target_initialized",
            "clone_verified",
            "restore_qualified",
        ]
        .iter()
        .all(|field| value[*field] == false))
}

pub(super) fn resume_names(target: &Path) -> Result<BTreeSet<String>> {
    names(
        target,
        r"^resume-(initialize|redis-markers|migrate)-.*\.json$",
    )
}

pub(super) fn descriptor_name(value: &Value) -> Result<String> {
    Path::new(json_io::text(&value["path"], "文件绑定路径")?)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| "文件绑定名称无效".into())
}

pub(super) fn artifact_bindings(bytes: &[u8]) -> Result<Vec<Value>> {
    #[derive(serde::Deserialize)]
    struct Build {
        #[serde(deserialize_with = "ordered_artifacts")]
        artifacts: Vec<Value>,
    }
    Ok(serde_json::from_slice::<Build>(bytes)?.artifacts)
}

fn ordered_artifacts<'de, D>(input: D) -> std::result::Result<Vec<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, MapAccess, Visitor};
    struct Artifacts;
    impl<'de> Visitor<'de> for Artifacts {
        type Value = Vec<Value>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("有序维护二进制对象")
        }
        fn visit_map<A: MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut keys = BTreeSet::new();
            let mut values = Vec::new();
            while let Some((key, value)) = map.next_entry::<String, Value>()? {
                if !keys.insert(key) {
                    return Err(de::Error::custom("维护二进制角色重复"));
                }
                values.push(value);
            }
            if keys
                != ["reset", "migrate", "tenant-data"]
                    .map(str::to_owned)
                    .into()
            {
                return Err(de::Error::custom("维护二进制角色不同"));
            }
            Ok(values)
        }
    }
    input.deserialize_map(Artifacts)
}
