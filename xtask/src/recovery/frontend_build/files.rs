//! 普通外部来源与持有文件的流式摘要；文档限额不作用于 dist 大文件。
use super::{fail, model};
use crate::{Result, check::consumer_context::HeldFile};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub(super) const LIMIT: u64 = 16 * 1024 * 1024;
pub(super) const RECEIPT: &str = ".vite/restore-build.json";

pub(super) fn directory(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    checked(&path, true, false, false)?.ok_or_else(|| fail("缺少普通目录"))
}

pub(super) fn ordinary(path: &Path, optional: bool, managed: bool) -> Result<Option<PathBuf>> {
    checked(path, false, optional, managed)
}
fn checked(path: &Path, directory: bool, optional: bool, managed: bool) -> Result<Option<PathBuf>> {
    let text = path.to_str().ok_or_else(|| fail("文件路径必须是 UTF8"))?;
    if !path.is_absolute() || text.contains(['\0', '\r', '\n']) {
        return Err(fail("文件必须使用不含 NUL/CR/LF 的绝对路径"));
    }
    let parts = path.components().collect::<Vec<_>>();
    let mut current = PathBuf::new();
    for (index, part) in parts.iter().enumerate() {
        match part {
            Component::ParentDir => {
                current.pop();
            }
            Component::CurDir => {}
            _ => current.push(part.as_os_str()),
        }
        if matches!(part, Component::Prefix(_)) {
            continue;
        }
        let leaf = index + 1 == parts.len();
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if super::super::clone::native::digest::linked(&metadata)
                    || if leaf && !directory {
                        !metadata.is_file()
                    } else {
                        !metadata.is_dir()
                    }
                {
                    return Err(fail("文件路径经过链接、重解析点或错误类型"));
                }
                let actual = fs::canonicalize(&current)
                    .map_err(|_| fail("无法确认真实来源；检查文件、目录及权限"))?;
                if !equivalent(&current, &actual) {
                    return Err(fail("真实路径与普通路径不一致；使用无别名的普通来源路径"));
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && optional
                    && (leaf || managed) =>
            {
                return Ok(None);
            }
            Err(_) => return Err(fail("必需文件或父目录不可读取；检查配置、产物与权限")),
        }
    }
    Ok(Some(current))
}

fn relative<'a>(root: &Path, path: &'a Path) -> Option<&'a Path> {
    let mut remaining = path.components();
    for expected in root.components() {
        if !same_component(expected, remaining.next()?) {
            return None;
        }
    }
    Some(remaining.as_path())
}

fn equivalent(left: &Path, right: &Path) -> bool {
    relative(left, right).is_some_and(|suffix| suffix.as_os_str().is_empty())
}

#[cfg(not(windows))]
fn same_component(left: Component<'_>, right: Component<'_>) -> bool {
    left == right
}

#[cfg(windows)]
fn same_component(left: Component<'_>, right: Component<'_>) -> bool {
    match (left, right) {
        (Component::Prefix(left), Component::Prefix(right)) => {
            same_prefix(left.kind(), right.kind())
        }
        (Component::Normal(left), Component::Normal(right)) => same_text(left, right),
        _ => left == right,
    }
}

#[cfg(windows)]
fn same_text(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    left.to_str()
        .zip(right.to_str())
        .is_some_and(|(left, right)| left.to_lowercase() == right.to_lowercase())
}

#[cfg(windows)]
fn same_prefix(left: std::path::Prefix<'_>, right: std::path::Prefix<'_>) -> bool {
    use std::path::Prefix;
    match (left, right) {
        (
            Prefix::Disk(left) | Prefix::VerbatimDisk(left),
            Prefix::Disk(right) | Prefix::VerbatimDisk(right),
        ) => left.eq_ignore_ascii_case(&right),
        (
            Prefix::UNC(left_server, left_share) | Prefix::VerbatimUNC(left_server, left_share),
            Prefix::UNC(right_server, right_share) | Prefix::VerbatimUNC(right_server, right_share),
        ) => same_text(left_server, right_server) && same_text(left_share, right_share),
        (Prefix::DeviceNS(left), Prefix::DeviceNS(right))
        | (Prefix::Verbatim(left), Prefix::Verbatim(right)) => same_text(left, right),
        _ => false,
    }
}

pub(super) fn document(path: &Path) -> Result<(StableFile, Value)> {
    let path = ordinary(path, false, false)?.ok_or_else(|| fail("缺少文档"))?;
    let mut held = StableFile::open(&path)?;
    let bytes = held.read()?;
    let value = parse(&bytes)?;
    held.verify()?;
    Ok((held, value))
}

pub(super) fn parse(bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() || bytes.len() as u64 > LIMIT {
        return Err(fail("JSON 文档为空或超过 16 MiB"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| fail("JSON 文档 UTF8 无效"))?;
    let value = super::super::clone::native::json::parse_fragment(text, "前端证明")
        .map_err(|_| fail("JSON 语法、重复键或数值无效"))?;
    if !value.is_object() {
        return Err(fail("JSON 文档必须是对象"));
    }
    Ok(value)
}

pub(super) fn descriptor(root: &Path, path: &Path, sha256: &str) -> Result<Value> {
    Ok(json!({"path":name(root,path)?,"sha256":sha256}))
}

pub(super) fn name(root: &Path, path: &Path) -> Result<String> {
    let path = relative(root, path).unwrap_or(path);
    let text = path
        .to_str()
        .ok_or_else(|| fail("文件路径必须是 UTF8"))?
        .replace('\\', "/");
    Ok(if text.is_empty() { ".".into() } else { text })
}

pub(in crate::recovery) struct StableFile {
    pub(super) held: HeldFile,
    modified: std::time::SystemTime,
    identity: (u64, u64),
}
impl StableFile {
    pub(in crate::recovery) fn open(path: &Path) -> Result<Self> {
        crate::recovery::runtime::dataset_authority::reads::file(path)?;
        let before = fs::symlink_metadata(path).map_err(|_| fail("无法读取文件身份"))?;
        let modified = before
            .modified()
            .map_err(|_| fail("无法读取文件修改时间"))?;
        let identity =
            super::super::clone::native::resources::common::device_inode_for(path, &before)?;
        let held = HeldFile::open(path)?;
        let file = Self {
            held,
            modified,
            identity,
        };
        if file.held.bytes != before.len() {
            return Err(fail("文件读取期间长度变化"));
        }
        file.verify()?;
        Ok(file)
    }
    pub(super) fn read(&mut self) -> Result<Vec<u8>> {
        let bytes = self.held.read(LIMIT)?;
        let repeated = self.held.read(LIMIT)?;
        self.verify()?;
        if bytes != repeated {
            return Err(fail("文档读取期间字节变化"));
        }
        Ok(bytes)
    }
    pub(in crate::recovery) fn facts(&self) -> Result<Value> {
        self.verify()?;
        let modified = match self.modified.duration_since(std::time::UNIX_EPOCH) {
            Ok(value) => value.as_nanos().to_string(),
            Err(error) => format!("-{}", error.duration().as_nanos()),
        };
        Ok(
            json!({"path":self.held.path,"bytes":self.held.bytes,"sha256":self.held.sha256,
            "device":self.identity.0,"inode":self.identity.1,"mtime_ns":modified}),
        )
    }
    pub(in crate::recovery) fn verify(&self) -> Result<()> {
        self.held.verify()?;
        let metadata =
            fs::symlink_metadata(&self.held.path).map_err(|_| fail("文件身份不可读取"))?;
        if metadata.modified().ok() != Some(self.modified)
            || metadata.len() != self.held.bytes
            || super::super::clone::native::resources::common::device_inode_for(
                &self.held.path,
                &metadata,
            )? != self.identity
        {
            return Err(fail("文件身份、长度或修改时间变化"));
        }
        Ok(())
    }
}

pub(crate) struct Artifacts {
    pub(crate) files: Value,
    held: Vec<StableFile>,
}
impl Artifacts {
    pub(crate) fn verify(&self) -> Result<()> {
        for file in &self.held {
            file.verify()?;
        }
        Ok(())
    }
}

pub(crate) fn artifacts(root: &Path) -> Result<Artifacts> {
    let root = directory(root, root)?;
    let dist = directory(&root, &root.join("dist"))?;
    let mut pending = vec![dist.clone()];
    let mut held = Vec::new();
    while let Some(directory) = pending.pop() {
        crate::recovery::runtime::dataset_authority::reads::directory(&directory)?;
        for entry in
            fs::read_dir(&directory).map_err(|_| fail("dist 枚举失败；检查产物目录权限"))?
        {
            let path = entry.map_err(|_| fail("dist 条目不可读取"))?.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| fail("dist 条目身份不可读取"))?;
            if super::super::clone::native::digest::linked(&metadata) {
                return Err(fail("dist 禁止链接或重解析点"));
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                if name(&dist, &path)? != RECEIPT {
                    held.push(StableFile::open(&path)?);
                }
            } else {
                return Err(fail("dist 只能包含普通文件和目录"));
            }
        }
    }
    let mut named = held
        .into_iter()
        .map(|file| Ok((name(&dist, &file.held.path)?, file)))
        .collect::<Result<Vec<_>>>()?;
    named.sort_by(|left, right| left.0.cmp(&right.0));
    let files = named
        .iter()
        .map(|(path, file)| json!({"path":path,"bytes":file.held.bytes,"sha256":file.held.sha256}))
        .collect::<Vec<_>>();
    let held = named.into_iter().map(|(_, file)| file).collect();
    if !["index.html", ".vite/manifest.json"]
        .iter()
        .all(|name| files.iter().any(|file| file["path"] == *name))
    {
        return Err(fail(
            "dist 缺少 index.html 或 .vite/manifest.json；重新生产构建",
        ));
    }
    let files = json!(files);
    model::artifact_records(&files)?;
    let artifacts = Artifacts { files, held };
    artifacts.verify()?;
    Ok(artifacts)
}
