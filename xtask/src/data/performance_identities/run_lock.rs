//! owner、打开句柄与路径身份同时匹配才释放锁；墓碑与标记永久保留。
use super::evidence;
use crate::{Result, recovery::clone::native::json as document};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub(super) struct RunLock {
    root: PathBuf,
    file: File,
    identity: same_file::Handle,
    bytes: Vec<u8>,
}
pub(super) fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| {
        id.get_version_num() == 4
            && id.get_variant() == uuid::Variant::RFC4122
            && id.to_string() == value
    })
}

impl RunLock {
    pub(super) fn acquire(root: &Path, plan: &Value, token: &str) -> Result<Self> {
        if !uuid(token) {
            return Err("身份锁 owner token 无效".into());
        }
        inventory(root)?;
        let path = root.join("lock");
        let value = json!({"format_version":1,"kind":"devex-identity-run-lock","pid":std::process::id(),"plan_sha256":plan["plan_sha256"],"owner_token":token});
        let bytes = evidence::encode(&value)?;
        let mut file = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&path)?;
        let identity = same_file::Handle::from_file(file.try_clone()?)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        let owner = Self {
            root: root.into(),
            file,
            identity,
            bytes,
        };
        owner.owned(&path)?;
        inventory(root)?;
        Ok(owner)
    }
    fn owned(&self, path: &Path) -> Result<()> {
        if self.identity != same_file::Handle::from_file(self.file.try_clone()?)?
            || self.identity != same_file::Handle::from_path(path)?
            || evidence::regular(path)?.as_deref() != Some(self.bytes.as_slice())
        {
            return Err("身份运行锁被替换；拒绝释放其他 owner 的锁".into());
        }
        Ok(())
    }
    pub(super) fn release(self) -> Result<()> {
        self.release_with(&mut |_, _| Ok(()))
    }
    pub(super) fn release_with(
        self,
        checkpoint: &mut dyn FnMut(&str, &Path) -> Result<()>,
    ) -> Result<()> {
        let path = self.root.join("lock");
        self.owned(&path)?;
        checkpoint("before-claim", &path)?;
        let directory = release_directory(&self.root)?;
        let id = uuid::Uuid::new_v4().to_string();
        let tombstone = directory.join(format!("{id}.lock"));
        let marker = directory.join(format!("{id}.released.json"));
        if tombstone.exists() || marker.exists() {
            return Err("身份锁随机释放名称已存在".into());
        }
        fs::rename(path, &tombstone)?;
        self.owned(&tombstone)?;
        checkpoint("claimed", &tombstone)?;
        let value = json!({"format_version":1,"kind":"devex-identity-run-lock-release","release_id":id,"tombstone_file":format!("{id}.lock"),"lock_sha256":evidence::hash(&self.bytes),"lock":lock_value(&self.bytes)?});
        evidence::immutable(&marker, &value)?;
        release_record(&directory, &id)?;
        checkpoint("released", &marker)?;
        Ok(())
    }
}

fn release_directory(root: &Path) -> Result<PathBuf> {
    let directory = root.join("lock-releases");
    match fs::create_dir(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(&directory)?;
    if !metadata.is_dir() || crate::recovery::clone::native::digest::linked(&metadata) {
        return Err("身份锁释放目录必须为真实目录".into());
    }
    Ok(directory)
}
fn lock_value(bytes: &[u8]) -> Result<Value> {
    let value = evidence::parse(bytes)?;
    document::exact(
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
    let hash = value["plan_sha256"].as_str().unwrap_or("");
    if value["format_version"] != 1
        || value["kind"] != "devex-identity-run-lock"
        || !value["pid"].as_u64().is_some_and(|id| id > 0)
        || hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !uuid(value["owner_token"].as_str().unwrap_or(""))
        || evidence::encode(&value)? != bytes
    {
        return Err("身份锁记录未绑定规范 owner".into());
    }
    Ok(value)
}
fn release_record(directory: &Path, id: &str) -> Result<Value> {
    let tombstone = directory.join(format!("{id}.lock"));
    let marker = directory.join(format!("{id}.released.json"));
    let bytes = evidence::regular(&tombstone)?.ok_or("身份锁缺少释放墓碑")?;
    let marker_bytes = evidence::regular(&marker)?.ok_or("身份锁释放未完成，拒绝开放互斥")?;
    let lock = lock_value(&bytes)?;
    let expected = json!({"format_version":1,"kind":"devex-identity-run-lock-release","release_id":id,"tombstone_file":format!("{id}.lock"),"lock_sha256":evidence::hash(&bytes),"lock":lock});
    if marker_bytes != evidence::encode(&expected)?
        || evidence::regular(&tombstone)?.as_deref() != Some(bytes.as_slice())
        || evidence::regular(&marker)?.as_deref() != Some(marker_bytes.as_slice())
    {
        return Err("身份锁释放标记、墓碑或 owner 不一致".into());
    }
    Ok(lock)
}
fn filenames(path: &Path) -> Result<BTreeSet<String>> {
    fs::read_dir(path)?
        .map(|entry| {
            Ok(entry?
                .file_name()
                .into_string()
                .map_err(|_| "身份释放文件名必须为 UTF-8")?)
        })
        .collect()
}
fn inventory(root: &Path) -> Result<()> {
    let directory = release_directory(root)?;
    let files = filenames(&directory)?;
    let mut ids = BTreeSet::new();
    for name in &files {
        let id = name
            .strip_suffix(".lock")
            .or_else(|| name.strip_suffix(".released.json"))
            .ok_or("身份锁释放目录有未知或未完成证据")?;
        if !uuid(id) {
            return Err("身份锁释放文件名不是规范 UUID".into());
        }
        ids.insert(id);
    }
    let mut owners = BTreeSet::new();
    for id in ids {
        let lock = release_record(&directory, id)?;
        if !owners.insert(lock["owner_token"].clone().to_string()) {
            return Err("同一身份 owner 有多个释放记录".into());
        }
    }
    if files != filenames(&directory)? {
        return Err("身份锁释放目录在核验期间变化".into());
    }
    Ok(())
}
