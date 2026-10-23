//! 验证当前原生程序与编译源闭包，再持有固定文件绑定。

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use super::{
    files::{self, HeldFile, Source},
    identity::Identity,
    python,
};
use crate::Result;

#[path = "../../frontend_origin/sources.rs"]
mod source_inventory;

const INVALID: &str = "origin 工具绑定无效：核对当前程序、完整编译来源清单和固定绑定文件，保留失败证据后重新编译同一来源工具。";
const LIMIT: u64 = 4 * 1024 * 1024;
const DESCRIBE_ARGUMENTS: [&str; 2] = ["__frontend-origin", "describe"];

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Executable {
    path: PathBuf,
    sha256: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Description {
    format_version: u32,
    kind: String,
    executable: Executable,
    sources: Vec<Source>,
}

pub(crate) struct OriginBinding {
    pub(super) binding: HeldFile,
    held: Vec<HeldFile>,
    evidence: PathBuf,
    source_root: PathBuf,
    source_paths: Vec<String>,
}

impl OriginBinding {
    pub(crate) fn verify(&self) -> Result<()> {
        let result = (|| {
            self.binding.verify()?;
            if source_inventory::discover(&self.source_root)? != self.source_paths {
                return Err(INVALID.into());
            }
            for file in &self.held {
                file.verify()?;
            }
            Ok(())
        })();
        if result.is_err() {
            super::record_failure(&self.evidence);
        }
        result
    }

    pub(crate) fn finish(self, result: Result<()>) -> Result<()> {
        let verification = self.verify();
        if result.is_err() {
            super::record_failure(&self.evidence);
        }
        super::finish_verified_result(result, verification)
    }
}

pub(super) fn prepare(root: &Path, evidence: &Path) -> Result<OriginBinding> {
    let executable = std::env::current_exe().map_err(|_| INVALID)?;
    let program = HeldFile::open(&executable)?;
    let output = python::output(
        root,
        python::command(root, &executable, &DESCRIBE_ARGUMENTS),
        None,
        Duration::from_secs(60),
    )?;
    if !output.status.success() {
        return Err(INVALID.into());
    }
    files::write_new(&evidence.join("origin-tool-described.json"), &output.stdout)?;
    let description = parse(&output.stdout)?;
    let mut held = validate_current(root, &description, &executable)?;
    if program.sha256 != description.executable.sha256 {
        return Err(INVALID.into());
    }
    held.push(program);
    let bytes = serde_json::to_vec(&description)?;
    let binding = cache(root, evidence, &description, &bytes)?;
    Ok(OriginBinding {
        binding,
        held,
        evidence: evidence.to_owned(),
        source_root: root.to_owned(),
        source_paths: description
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect(),
    })
}

fn parse(bytes: &[u8]) -> Result<Description> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| INVALID)?;
    if !value.is_object()
        || !value["executable"].is_object()
        || !value["sources"]
            .as_array()
            .is_some_and(|sources| sources.iter().all(serde_json::Value::is_object))
    {
        return Err(INVALID.into());
    }
    let description: Description = serde_json::from_slice(bytes).map_err(|_| INVALID)?;
    if description.format_version != 1
        || description.kind != "frontend-origin-tool"
        || !description.executable.path.is_absolute()
        || !files::digest_valid(&description.executable.sha256)
        || description.sources.is_empty()
    {
        return Err(INVALID.into());
    }
    for (index, source) in description.sources.iter().enumerate() {
        if !files::digest_valid(&source.sha256)
            || source.path.contains(['\\', ':', '\0', '\r', '\n'])
            || source
                .path
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
            || index > 0 && description.sources[index - 1].path >= source.path
        {
            return Err(INVALID.into());
        }
    }
    Ok(description)
}

fn validate_executable(description: &Description) -> Result<HeldFile> {
    let mut executable = HeldFile::open(&description.executable.path)?;
    executable.verify_binary()?;
    if executable.sha256 != description.executable.sha256 {
        return Err(INVALID.into());
    }
    Ok(executable)
}

fn same_current_program(previous: &Description, current: &HeldFile) -> Result<bool> {
    files::no_links(&previous.executable.path)?;
    files::no_links(&current.path)?;
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        for path in [&previous.executable.path, &current.path] {
            if !matches!(path.components().next(), Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_)))
            {
                return Ok(false);
            }
        }
    }
    if fs::canonicalize(&previous.executable.path).map_err(|_| INVALID)?
        != fs::canonicalize(&current.path).map_err(|_| INVALID)?
    {
        return Ok(false);
    }
    let previous_identity = Identity::open(&previous.executable.path, false)?;
    let current_identity = Identity::open(&current.path, false)?;
    if !previous_identity.same(&current_identity) {
        return Err(INVALID.into());
    }
    previous_identity.verify(&previous.executable.path)?;
    current.verify()?;
    Ok(true)
}

fn validate_current(
    root: &Path,
    description: &Description,
    program: &Path,
) -> Result<Vec<HeldFile>> {
    files::no_links(&description.executable.path)?;
    files::no_links(program)?;
    if fs::canonicalize(&description.executable.path).map_err(|_| INVALID)?
        != fs::canonicalize(program).map_err(|_| INVALID)?
    {
        return Err(INVALID.into());
    }
    let actual = source_inventory::discover(root).map_err(|_| INVALID)?;
    if !actual.iter().map(String::as_str).eq(description
        .sources
        .iter()
        .map(|source| source.path.as_str()))
    {
        return Err(INVALID.into());
    }
    let mut held = vec![validate_executable(description)?];
    for source in &description.sources {
        let file = HeldFile::open(&root.join(&source.path))?;
        if file.sha256 != source.sha256 {
            return Err(INVALID.into());
        }
        held.push(file);
    }
    Ok(held)
}

struct CacheLock {
    path: PathBuf,
    file: Option<HeldFile>,
    parent: Identity,
}

impl CacheLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        files::raw_path(&path)?;
        let parent_path = path.parent().ok_or(files::FILE_ERROR)?;
        files::no_links(parent_path)?;
        let parent = Identity::open(parent_path, true)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let mut file = options
            .open(&path)
            .map_err(|_| "origin 绑定正被另一控制器持有或锁文件未知：保留现场并核对原控制器。")?;
        let ownership = serde_json::to_vec(&serde_json::json!({"format_version":1,
            "kind":"consumer-origin-binding-lock", "pid":std::process::id(),
            "nonce":uuid::Uuid::new_v4().to_string()}))?;
        file.write_all(&ownership)?;
        file.sync_all()?;
        let file = HeldFile::from_created(&path, file)?;
        if file.sha256 != files::hash(&ownership) {
            return Err(files::FILE_ERROR.into());
        }
        parent.verify(parent_path)?;
        Ok(Self {
            path,
            file: Some(file),
            parent,
        })
    }

    fn release(mut self) -> Result<()> {
        let file = self.file.take().ok_or(files::FILE_ERROR)?;
        self.parent
            .verify(self.path.parent().ok_or(files::FILE_ERROR)?)?;
        file.verify()?;
        drop(file);
        fs::remove_file(&self.path).map_err(|_| files::FILE_ERROR)?;
        Ok(())
    }
}

fn cache(
    root: &Path,
    evidence: &Path,
    description: &Description,
    bytes: &[u8],
) -> Result<HeldFile> {
    if &parse(bytes)? != description {
        return Err(INVALID.into());
    }
    let current = validate_executable(description)?;
    let path = root.join(".local-tests/frontend-origin-tool.json");
    let directory = root.join(".local-tests/consumer-contexts");
    files::directory(&directory)?;
    let lock = CacheLock::acquire(directory.join("origin-binding.lock"))?;
    if files::exists(&path)? {
        let mut old = HeldFile::open(&path)?;
        let old_bytes = old.read(LIMIT)?;
        let previous = parse(&old_bytes)?;
        // 同一路径上的当前程序已独立验证；重建后只允许旧摘要落后的明确情形。
        let _old_executable = if previous.executable.sha256 != current.sha256
            && same_current_program(&previous, &current)?
        {
            None
        } else {
            Some(validate_executable(&previous)?)
        };
        if &previous == description {
            old.verify()?;
            current.verify()?;
            lock.release()?;
            return Ok(old);
        }
        current.verify()?;
        files::write_new(
            &evidence.join(format!("origin-tool-{}.json", old.sha256)),
            &old_bytes,
        )?;
        old.verify()?;
        drop(old);
    }
    let mut replacement = tempfile::NamedTempFile::new_in(root.join(".local-tests"))?;
    replacement.write_all(bytes)?;
    replacement.as_file().sync_all()?;
    current.verify()?;
    replacement.persist(&path).map_err(|_| files::FILE_ERROR)?;
    let bound = HeldFile::open(&path)?;
    if bound.sha256 != files::hash(bytes) {
        return Err(INVALID.into());
    }
    current.verify()?;
    lock.release()?;
    Ok(bound)
}

#[cfg(test)]
#[path = "../../../tests/internal/consumer_context/origin_tests.rs"]
mod tests;
