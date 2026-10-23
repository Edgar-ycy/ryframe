use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use uuid::Uuid;

use super::super::{json as json_io, resources::common};
use crate::Result;

pub(super) fn recover(request: &Value) -> Result<Value> {
    let root = json_io::exact(
        request,
        &[
            "backend",
            "identity_state",
            "identity",
            "owner_token",
            "plan_sha256",
        ],
        "身份锁恢复请求",
    )?;
    let backend = Path::new(json_io::text(&root["backend"], "后端目录")?);
    let state = PathBuf::from(json_io::text(&root["identity_state"], "身份锁目录")?);
    if !state.is_absolute()
        || state.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || !state.canonicalize()?.starts_with(backend.canonicalize()?)
        || super::super::digest::linked(&fs::symlink_metadata(&state)?)
    {
        return Err("身份锁根目录不是后端范围内的规范绝对目录".into());
    }
    let identity = common::validate_process_identity(&root["identity"])?;
    let owner = json_io::text(&root["owner_token"], "身份锁 owner")?;
    if Uuid::parse_str(owner)
        .map_err(|_| "身份锁 owner 无效")?
        .get_version_num()
        != 4
    {
        return Err("身份锁 owner 必须为 UUID v4".into());
    }
    let plan = json_io::text(&root["plan_sha256"], "身份计划摘要")?;
    if !sha256(plan) {
        return Err("身份计划摘要无效".into());
    }
    let expected = json!({"format_version":1,"kind":"devex-identity-run-lock","pid":identity["pid"],"plan_sha256":plan,"owner_token":owner});
    let encoded = encode(&expected)?;
    let lock = state.join("lock");
    let releases = release_dir(&state)?;
    let mut matching = None;
    let mut owners = BTreeSet::new();
    let inventory = inventory(&releases)?;
    let mut incomplete_other = false;
    for (id, files) in inventory {
        let tombstone = releases.join(format!("{id}.lock"));
        if !files.contains("lock") {
            return Err("身份锁 release 记录缺少墓碑".into());
        }
        let content = regular(&tombstone)?;
        let value = lock_value(&content)?;
        let marker = releases.join(format!("{id}.released.json"));
        let pending = releases.join(format!("{id}.released.json.pending"));
        let marker_value = release_marker(&id, &tombstone, &value, &content)?;
        let released = files.contains("marker");
        let pending_exists = files.contains("pending");
        if released && regular(&marker)? != marker_value {
            return Err("身份锁 released marker 与墓碑不一致".into());
        }
        if pending_exists && regular(&pending)? != marker_value {
            return Err("身份锁 released pending 与墓碑不一致".into());
        }
        let owner_value = json_io::text(&value["owner_token"], "身份锁 release owner")?;
        if !owners.insert(owner_value.to_owned()) {
            return Err("同一身份锁 owner 存在多个 release 记录".into());
        }
        if value == expected {
            if matching.is_some() {
                return Err("同一身份锁存在多个 release 记录".into());
            }
            matching = Some((marker, pending, marker_value, released));
        } else if !released || pending_exists {
            incomplete_other = true;
        }
    }
    if incomplete_other {
        return Err("身份锁存在不属于固定 owner 的未完成 release".into());
    }
    if let Some((marker, pending, marker_value, released)) = matching {
        publish_marker(&marker, &pending, &marker_value)?;
        return Ok(json!({"released":!released,"idempotent":released}));
    }
    let first = match snapshot(&lock)? {
        Some(v) => v,
        None => return Ok(json!({"released":false,"idempotent":true})),
    };
    if first != encoded {
        return Err("身份锁不属于已确认死亡的固定原生、计划和 owner".into());
    }
    if snapshot(&lock)?.as_ref() != Some(&first) {
        return Err("身份锁 claim 前被替换或修改".into());
    }
    let id = Uuid::new_v4();
    let tombstone = releases.join(format!("{id}.lock"));
    let marker = releases.join(format!("{id}.released.json"));
    let pending = releases.join(format!("{id}.released.json.pending"));
    fs::rename(&lock, &tombstone)?;
    if regular(&tombstone)? != first {
        return Err("身份锁 claim 时被替换；replacement 已保留在墓碑".into());
    }
    let marker_value = release_marker(&id, &tombstone, &expected, &first)?;
    publish_marker(&marker, &pending, &marker_value)?;
    Ok(json!({"released":true,"idempotent":false}))
}

fn release_dir(root: &Path) -> Result<PathBuf> {
    let path = root.join("lock-releases");
    if path.exists() {
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || super::super::digest::linked(&metadata) {
            return Err("身份锁 releases 不是目录".into());
        }
    } else {
        fs::create_dir(&path)?
    }
    Ok(path)
}
fn snapshot(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_file() && !super::super::digest::linked(&m) => {
            Ok(Some(fs::read(path)?))
        }
        Ok(_) => Err("身份锁必须是无链接普通文件".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn regular(path: &Path) -> Result<Vec<u8>> {
    snapshot(path)?.ok_or_else(|| "身份锁 release 文件缺失".into())
}
fn encode(value: &Value) -> Result<Vec<u8>> {
    let mut out = serde_json::to_string_pretty(value)?;
    out.push('\n');
    Ok(out.into_bytes())
}
fn release_marker(id: &Uuid, tombstone: &Path, lock: &Value, content: &[u8]) -> Result<Vec<u8>> {
    encode(
        &json!({"format_version":1,"kind":"devex-identity-run-lock-release","release_id":id.to_string(),"tombstone_file":tombstone.file_name().and_then(|v|v.to_str()).ok_or("身份锁墓碑名称无效")?,"lock_sha256":sha2_hex(content),"lock":lock}),
    )
}
fn publish_marker(marker: &Path, pending: &Path, content: &[u8]) -> Result<()> {
    if marker.exists() {
        if regular(marker)? != content {
            return Err("身份锁 released marker 与墓碑不一致".into());
        }
        return Ok(());
    }
    if pending.exists() {
        if regular(pending)? != content {
            return Err("身份锁 released pending 与墓碑不一致".into());
        }
    } else {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(pending)?;
        file.write_all(content)?;
        file.sync_all()?;
    }
    match fs::hard_link(pending, marker) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    if regular(marker)? != content {
        return Err("身份锁 released marker 发布后不一致".into());
    }
    if pending.exists() && regular(pending)? == content {
        fs::remove_file(pending)?
    }
    Ok(())
}
fn sha2_hex(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(content))
}
fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn lock_value(content: &[u8]) -> Result<Value> {
    if content.len() > 4096 {
        return Err("身份锁内容超过 4096 字节".into());
    }
    let text = std::str::from_utf8(content).map_err(|_| "身份锁不是 UTF-8")?;
    let value = json_io::parse_fragment(text, "身份锁")?;
    json_io::exact(
        &value,
        &[
            "format_version",
            "kind",
            "pid",
            "plan_sha256",
            "owner_token",
        ],
        "身份锁",
    )?;
    Ok(value)
}

fn inventory(directory: &Path) -> Result<BTreeMap<Uuid, BTreeSet<&'static str>>> {
    let mut records = BTreeMap::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let (raw, kind) = if let Some(value) = name.strip_suffix(".released.json.pending") {
            (value, "pending")
        } else if let Some(value) = name.strip_suffix(".released.json") {
            (value, "marker")
        } else if let Some(value) = name.strip_suffix(".lock") {
            (value, "lock")
        } else {
            return Err("身份锁释放目录存在未知证据".into());
        };
        let id = Uuid::parse_str(raw).map_err(|_| "身份锁 release 名称无效")?;
        if id.get_version_num() != 4
            || !records.entry(id).or_insert_with(BTreeSet::new).insert(kind)
        {
            return Err("身份锁 release 名称无效或重复".into());
        }
    }
    Ok(records)
}
