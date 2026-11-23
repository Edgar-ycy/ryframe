use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::{Value, json};

use super::{
    catalog::{self, Catalog},
    digest::{self, FileDigest},
    held_plan::HeldPlanObjects,
    json::{array, boolean, exact, object, text},
    metadata,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

pub(crate) const BUCKETS: &[&str] = &["uploads", "avatar", "exports", "imports", "config-packages"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Database {
    pub(crate) key: String,
    pub(crate) kind: String,
    pub(crate) mode: String,
    pub(crate) server_uuid: String,
    pub(crate) database: String,
    pub(crate) schema_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactBinding {
    pub(crate) file: String,
    pub(crate) digest: FileDigest,
}

pub(crate) struct Objects {
    pub(crate) entries: Vec<Value>,
    pub(crate) index: BTreeMap<(String, String), ArtifactBinding>,
}

pub(crate) struct ValidatedInput {
    pub(crate) artifact_root: PathBuf,
    pub(crate) old: BTreeMap<String, Database>,
    pub(crate) new: BTreeMap<String, Database>,
    pub(crate) control: Catalog,
    pub(crate) tenant: Catalog,
    pub(crate) sources: Vec<PathBuf>,
    pub(crate) objects: Objects,
}

struct Side {
    scope: String,
    endpoint: String,
    databases: BTreeMap<String, Database>,
}

pub(crate) fn validate(value: &Value, backend: &Path) -> Result<ValidatedInput> {
    validate_with_objects(value, backend, None)
}

pub(super) fn validate_with_objects(
    value: &Value,
    backend: &Path,
    held: Option<&HeldPlanObjects>,
) -> Result<ValidatedInput> {
    let root = exact(
        value,
        &[
            "format_version",
            "copy_id",
            "copy_stage",
            "target_schedule_actions",
            "app_env",
            "artifact_root",
            "source_snapshot",
            "source",
            "target",
            "evidence",
            "databases",
            "objects",
            "execution_backend",
        ],
        "复制输入",
    )?;
    if !root
        .get("format_version")
        .is_some_and(|value| value.as_u64() == Some(1))
        || root.get("app_env").and_then(Value::as_str) != Some("test")
    {
        return Err("开发复制仅接受 test 环境当前结构".into());
    }
    name(text(root.get("copy_id").unwrap(), "copy_id")?)?;
    validate_snapshot(root.get("source_snapshot").unwrap())?;
    let old_side = side(root.get("source").unwrap(), false)?;
    let new_side = side(root.get("target").unwrap(), true)?;
    validate_sides(&old_side, &new_side)?;
    let artifact_root = artifact_root(&value["artifact_root"], backend)?;

    let execution = root.get("execution_backend").unwrap();
    let catalog_root = if execution.is_null() {
        backend.to_path_buf()
    } else {
        let value = execution
            .as_str()
            .ok_or("输入清单的执行工作树必须是绝对路径或空值")?;
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err("输入清单的执行工作树必须是绝对路径或空值".into());
        }
        path
    };
    let loaded = catalog::load(&catalog_root)?;
    let evidence = exact(
        root.get("evidence").unwrap(),
        &["source_stopped", "target_initialized", "target_stopped"],
        "复制证据",
    )?;
    for binding in evidence.values() {
        bound_file(&artifact_root, binding)?;
    }
    let object_declaration = root.get("objects").unwrap();
    if let Some(held) = held {
        held.require_declaration(&artifact_root, object_declaration)?;
    }
    let objects = object_plan(
        object_declaration,
        &artifact_root,
        &old_side.scope,
        &new_side.scope,
        held,
    )?;
    Ok(ValidatedInput {
        artifact_root,
        old: old_side.databases,
        new: new_side.databases,
        control: loaded.control,
        tenant: loaded.tenant,
        sources: loaded.sources,
        objects,
    })
}

pub(super) fn artifact_root(value: &Value, backend: &Path) -> Result<PathBuf> {
    let path = Path::new(text(value, "artifact_root")?);
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(|error| format!("复制证据根目录无效：{error}").into())
}

pub(crate) fn bound_file(root: &Path, value: &Value) -> Result<ArtifactBinding> {
    let binding = artifact_descriptor(value)?;
    let path = digest::safe_file(root, &binding.file)?;
    if digest::file(&path)? != binding.digest {
        return Err("复制证据已变化或未完整写入".into());
    }
    Ok(binding)
}

pub(super) fn artifact_descriptor(value: &Value) -> Result<ArtifactBinding> {
    let binding = exact(value, &["file", "bytes", "sha256"], "复制证据绑定")?;
    let file = text(binding.get("file").unwrap(), "证据文件")?.to_owned();
    let bytes = binding
        .get("bytes")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or("证据大小无效")?;
    let sha256 = digest_shape(binding.get("sha256").unwrap())?.to_owned();
    Ok(ArtifactBinding {
        file,
        digest: FileDigest { bytes, sha256 },
    })
}

pub(super) fn bound_object(
    root: &Path,
    value: &Value,
    held: Option<&HeldPlanObjects>,
) -> Result<ArtifactBinding> {
    match held {
        Some(held) => held.bound_object(root, value),
        None => bound_file(root, value),
    }
}

fn object_plan(
    value: &Value,
    root: &Path,
    source_scope: &str,
    target_scope: &str,
    held: Option<&HeldPlanObjects>,
) -> Result<Objects> {
    let buckets = array(value, "objects")?;
    if buckets.len() != BUCKETS.len() {
        return Err("必须完整列出五类对象桶，包括空桶".into());
    }
    let mut seen_buckets = BTreeSet::new();
    let mut seen_keys = BTreeSet::new();
    let mut entries = Vec::new();
    let mut index = BTreeMap::new();
    for bucket in buckets {
        let bucket = exact(bucket, &["bucket", "entries"], "对象桶")?;
        let name = text(bucket.get("bucket").unwrap(), "对象桶名")?;
        let items = array(bucket.get("entries").unwrap(), "对象条目")?;
        if !BUCKETS.contains(&name) || !seen_buckets.insert(name) {
            return Err("对象桶缺失或重复".into());
        }
        for item in items {
            let item = exact(item, &["key", "artifact", "metadata"], "对象条目")?;
            let key = text(item.get("key").unwrap(), "对象 key")?;
            let prefix = format!("{source_scope}/");
            let logical = key.strip_prefix(&prefix).unwrap_or("");
            let target_key = format!("{target_scope}/{logical}");
            if logical.is_empty()
                || logical == ".ryframe-owner"
                || logical.contains('\\')
                || logical
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || logical
                    .chars()
                    .any(|character| (character as u32) < 32 || character == '\u{7f}')
                || key.len() > 1024
                || target_key.len() > 1024
                || !seen_keys.insert((name.to_owned(), logical.to_owned()))
            {
                return Err("对象 key 越过精确来源前缀、包含 owner 或重复".into());
            }
            let metadata = metadata::copy(item.get("metadata").unwrap())?;
            let artifact = bound_object(root, item.get("artifact").unwrap(), held)?;
            index.insert((name.to_owned(), logical.to_owned()), artifact.clone());
            entries.push(json!({
                "bucket": name,
                "source_key": key,
                "target_key": target_key,
                "artifact": {
                    "file": artifact.file,
                    "bytes": artifact.digest.bytes,
                    "sha256": artifact.digest.sha256,
                },
                "metadata": metadata,
            }));
        }
    }
    Ok(Objects { entries, index })
}

fn side(value: &Value, target: bool) -> Result<Side> {
    if target {
        exact(
            value,
            &[
                "scope_id",
                "object_endpoint",
                "databases",
                "state",
                "ever_started",
            ],
            "复制目标",
        )?;
    } else {
        exact(
            value,
            &["scope_id", "object_endpoint", "databases"],
            "复制来源",
        )?;
    }
    let object = object(value, "复制侧")?;
    let scope = name(text(object.get("scope_id").unwrap(), "scope_id")?)?.to_owned();
    let endpoint = text(object.get("object_endpoint").unwrap(), "object_endpoint")?.to_owned();
    endpoint_url(&endpoint)?;
    if target
        && (object.get("state").and_then(Value::as_str) != Some("new_initialized")
            || object.get("ever_started").and_then(Value::as_bool) != Some(false))
    {
        return Err("目标必须声明为本次新初始化且从未运行的资源".into());
    }
    let databases = array(object.get("databases").unwrap(), "复制数据库")?;
    if databases.is_empty() {
        return Err("必须明确列出源和目标数据库".into());
    }
    let mut result = BTreeMap::new();
    let mut physical = HashSet::new();
    let database_pattern = Regex::new(r"^[a-z0-9_]{1,64}$")?;
    let uuid_pattern = Regex::new(r"^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$")?;
    for database in databases {
        let database = parse_database(database, &scope, &database_pattern, &uuid_pattern)?;
        if result.contains_key(&database.key)
            || !physical.insert((
                database.server_uuid.clone(),
                database.database.to_ascii_lowercase(),
            ))
        {
            return Err("数据库键或物理身份重复".into());
        }
        result.insert(database.key.clone(), database);
    }
    if result
        .values()
        .filter(|database| database.kind == "combined")
        .count()
        != 1
    {
        return Err("复制必须恰有一个完整 combined 控制库".into());
    }
    Ok(Side {
        scope,
        endpoint,
        databases: result,
    })
}

fn parse_database(
    value: &Value,
    scope: &str,
    database_pattern: &Regex,
    uuid_pattern: &Regex,
) -> Result<Database> {
    let database = exact(
        value,
        &[
            "key",
            "kind",
            "mode",
            "server_uuid",
            "database",
            "schema_sha256",
            "ownership",
        ],
        "数据库声明",
    )?;
    let key = name(text(database.get("key").unwrap(), "数据库键")?)?.to_owned();
    let kind = text(database.get("kind").unwrap(), "数据库类型")?.to_owned();
    let mode = text(database.get("mode").unwrap(), "数据库模式")?.to_owned();
    let server = text(database.get("server_uuid").unwrap(), "server_uuid")?;
    let name_value = text(database.get("database").unwrap(), "database")?;
    let schema = digest_shape(database.get("schema_sha256").unwrap())?.to_owned();
    if !database_pattern.is_match(name_value)
        || !uuid_pattern.is_match(server)
        || !matches!(kind.as_str(), "combined" | "tenant")
        || !matches!(mode.as_str(), "shared" | "dedicated")
        || (kind == "combined" && mode != "shared")
    {
        return Err("数据库物理身份或类型无效".into());
    }
    let kinds = if kind == "combined" {
        &["control", "tenant-data"][..]
    } else {
        &["tenant-data"][..]
    };
    let expected = kinds
        .iter()
        .map(|resource| {
            json!({
                "resource_kind": resource,
                "scope_id": scope,
                "marker": format!("ryframe-owner:v1:{scope}:{resource}"),
            })
        })
        .collect::<Vec<_>>();
    if database.get("ownership").and_then(Value::as_array) != Some(&expected) {
        return Err("必须完整声明各侧自己的精确 ownership，不能复制来源 owner".into());
    }
    Ok(Database {
        key,
        kind,
        mode,
        server_uuid: server.to_owned(),
        database: name_value.to_owned(),
        schema_sha256: schema,
    })
}

fn validate_sides(old: &Side, new: &Side) -> Result<()> {
    if old.scope == new.scope
        || old.databases.keys().collect::<BTreeSet<_>>()
            != new.databases.keys().collect::<BTreeSet<_>>()
    {
        return Err("必须同逻辑目标、不同物理 scope".into());
    }
    let old_physical = old
        .databases
        .values()
        .map(|database| (database.server_uuid.as_str(), database.database.as_str()))
        .collect::<HashSet<_>>();
    if new.databases.values().any(|database| {
        old_physical.contains(&(database.server_uuid.as_str(), database.database.as_str()))
    }) {
        return Err("新目标不能包含任何来源数据库".into());
    }
    for (key, database) in &old.databases {
        let target = &new.databases[key];
        if database.kind != target.kind
            || database.mode != target.mode
            || database.schema_sha256 != target.schema_sha256
        {
            return Err("两侧 schema、逻辑目标和模式必须相同".into());
        }
    }
    let _ = &old.endpoint;
    Ok(())
}

fn validate_snapshot(value: &Value) -> Result<()> {
    let value = exact(value, &["head", "clean", "worktree_sha256"], "源码快照")?;
    let head = text(value.get("head").unwrap(), "源码 head")?;
    boolean(value.get("clean").unwrap(), "源码 clean")?;
    digest_shape(value.get("worktree_sha256").unwrap())?;
    if !Regex::new(r"^[a-f0-9]{40}$")?.is_match(head) {
        return Err("必须准确记录开发源码快照与 dirty 状态".into());
    }
    Ok(())
}

pub(crate) fn name(value: &str) -> Result<&str> {
    let bytes = value.as_bytes();
    if (3..=48).contains(&bytes.len())
        && matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9')
        && bytes[1..]
            .iter()
            .all(|&byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
    {
        Ok(value)
    } else {
        Err("开发复制名称无效".into())
    }
}

pub(crate) fn digest_shape(value: &Value) -> Result<&str> {
    let value = text(value, "SHA-256")?;
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'f' | b'0'..=b'9'))
    {
        Ok(value)
    } else {
        Err("必须绑定真实 SHA-256".into())
    }
}

fn endpoint_url(value: &str) -> Result<()> {
    let rest = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
        .ok_or("恢复运行收据只接受明确的本机 loopback 端点")?;
    if rest.contains(['?', '#', '@']) {
        return Err("恢复运行收据只接受明确的本机 loopback 端点".into());
    }
    let (host, port) = if let Some(rest) = rest.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or("恢复运行收据只接受明确的本机 loopback 端点")?;
        if &rest[..end] != "::1" {
            return Err("恢复运行收据只接受明确的本机 loopback 端点".into());
        }
        let suffix = &rest[end + 1..];
        ("::1", suffix.strip_prefix(':'))
    } else {
        let (host, port) = rest
            .split_once(':')
            .map_or((rest, None), |(host, port)| (host, Some(port)));
        if host != "127.0.0.1" {
            return Err("恢复运行收据只接受明确的本机 loopback 端点".into());
        }
        ("127.0.0.1", port)
    };
    let _ = host;
    if port.is_some_and(|port| {
        port.is_empty()
            || !port.bytes().all(|byte| byte.is_ascii_digit())
            || port.parse::<u16>().is_err()
    }) {
        return Err("恢复运行收据只接受明确的本机 loopback 端点".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_name_matches_original_regex() {
        let reference = Regex::new(r"^[a-z0-9][a-z0-9_-]{2,47}$").unwrap();
        let lengths = (0..=50).map(|length| "a".repeat(length));
        let characters = (0u8..=127)
            .map(char::from)
            .chain(['é', '全', '１', '\u{0085}', '\u{2028}']);
        let spellings = characters.flat_map(|character| {
            [
                format!("{character}a0"),
                format!("a{character}0"),
                format!("a0{character}"),
            ]
        });
        for value in lengths.chain(spellings) {
            let expected = if reference.is_match(&value) {
                Ok(value.as_str())
            } else {
                Err("开发复制名称无效".to_owned())
            };
            assert_eq!(
                name(&value).map_err(|error| error.to_string()),
                expected,
                "{value:?}"
            );
        }
    }

    #[test]
    fn copy_digest_shape_matches_original_regex() {
        let reference = Regex::new(r"^[a-f0-9]{64}$").unwrap();
        let lengths = (0..=66).map(|length| "a".repeat(length));
        let characters = (0u8..=127)
            .map(char::from)
            .chain(['é', '全', '１', '\u{0085}', '\u{2028}']);
        let spellings = characters.flat_map(|character| {
            let tail = "a".repeat(64 - character.len_utf8());
            [format!("{character}{tail}"), format!("{tail}{character}")]
        });
        for value in lengths.chain(spellings).map(Value::String).chain([
            Value::Null,
            json!(true),
            json!(64),
            json!([]),
            json!({}),
        ]) {
            let expected = text(&value, "SHA-256").and_then(|value| {
                if reference.is_match(value) {
                    Ok(value)
                } else {
                    Err("必须绑定真实 SHA-256".into())
                }
            });
            assert_eq!(
                digest_shape(&value).map_err(|error| error.to_string()),
                expected.map_err(|error| error.to_string()),
                "{value:?}"
            );
        }
    }
}
