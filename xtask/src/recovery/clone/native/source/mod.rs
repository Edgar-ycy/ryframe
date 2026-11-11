use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{digest, json as native_json};
use crate::{Result, recovery::runtime::dataset_authority::reads, source_scan};

mod domains;
pub(crate) mod protocol;
mod root;
use root::Root;

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/source/tests.rs"]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileRecord {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceSnapshot {
    pub(crate) head: String,
    pub(crate) patch_sha256: String,
    pub(crate) files: Vec<FileRecord>,
    pub(crate) clean: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceBinding {
    pub(crate) snapshot: SourceSnapshot,
    pub(crate) worktree_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceGuard {
    pub(crate) head: String,
    pub(crate) index_sha256: String,
    pub(crate) modes_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Inventory {
    pub(crate) source: SourceBinding,
    pub(crate) files: Vec<FileRecord>,
    pub(crate) guard: SourceGuard,
}

pub(crate) fn current(root: &Path) -> Result<Value> {
    execution_source(&capture(root)?)
}

pub(crate) fn capture(root: &Path) -> Result<Inventory> {
    let root = Root::open(root)?;
    let captured: Result<Inventory> = (|| {
        let captured = capture_observed(
            || snapshot(&root),
            |head| worktree_fingerprint(&root, head),
            || file_inventory(&root),
        )?;
        crate::recovery::runtime::dataset_authority::reads::source(root.original(), &captured)?;
        Ok(captured)
    })();
    root.finish(captured)
}

fn capture_observed(
    mut snapshot: impl FnMut() -> Result<SourceSnapshot>,
    mut worktree_fingerprint: impl FnMut(&str) -> Result<String>,
    mut file_inventory: impl FnMut() -> Result<(Vec<FileRecord>, SourceGuard)>,
) -> Result<Inventory> {
    let before = snapshot()?;
    let fingerprint = worktree_fingerprint(&before.head)?;
    let (files, guard) = file_inventory()?;
    if snapshot()? != before
        || worktree_fingerprint(&before.head)? != fingerprint
        || guard.head != before.head
    {
        return Err("采集指纹期间完整源码发生变化".into());
    }
    Ok(Inventory {
        source: SourceBinding {
            snapshot: before,
            worktree_fingerprint: fingerprint,
        },
        files,
        guard,
    })
}

pub(crate) fn validate_inventory(value: &Value) -> Result<Inventory> {
    let object = native_json::exact(value, &["source", "files", "guard"], "完整来源清单")?;
    let source = native_json::exact(
        object.get("source").unwrap(),
        &["snapshot", "worktree_fingerprint"],
        "完整来源快照",
    )?;
    let snapshot = native_json::exact(
        source.get("snapshot").unwrap(),
        &["head", "patch_sha256", "files", "clean"],
        "完整来源 Git 快照",
    )?;
    let head = digest_value(snapshot.get("head").unwrap(), 40, "源码 HEAD")?;
    let patch_sha256 = digest_value(snapshot.get("patch_sha256").unwrap(), 64, "源码补丁摘要")?;
    let clean = native_json::boolean(snapshot.get("clean").unwrap(), "源码 clean 标志")?;
    let files = parse_files(snapshot.get("files").unwrap(), "未跟踪来源文件")?;
    let fingerprint = native_json::text(source.get("worktree_fingerprint").unwrap(), "工作区指纹")?;
    if !Regex::new(r"^sha256:[a-f0-9]{64}$")?.is_match(fingerprint) {
        return Err("完整来源 Git 快照内容无效".into());
    }
    let inventory_files = parse_files(object.get("files").unwrap(), "完整来源文件")?;
    let guard = native_json::exact(
        object.get("guard").unwrap(),
        &["head", "index_sha256", "modes_sha256"],
        "完整来源 Git guard",
    )?;
    if native_json::text(guard.get("head").unwrap(), "guard HEAD")? != head {
        return Err("完整来源 Git guard 无效".into());
    }
    let index_sha256 = digest_value(guard.get("index_sha256").unwrap(), 64, "Git 索引摘要")?;
    let modes_sha256 = digest_value(guard.get("modes_sha256").unwrap(), 64, "Git 模式摘要")?;
    Ok(Inventory {
        source: SourceBinding {
            snapshot: SourceSnapshot {
                head,
                patch_sha256,
                files,
                clean,
            },
            worktree_fingerprint: fingerprint.to_owned(),
        },
        files: inventory_files,
        guard: SourceGuard {
            head: native_json::text(guard.get("head").unwrap(), "guard HEAD")?.to_owned(),
            index_sha256,
            modes_sha256,
        },
    })
}

pub(crate) fn inventory_value(inventory: &Inventory) -> Value {
    json!({
        "source": binding_value(&inventory.source),
        "files": files_value(&inventory.files),
        "guard": {
            "head": inventory.guard.head,
            "index_sha256": inventory.guard.index_sha256,
            "modes_sha256": inventory.guard.modes_sha256,
        },
    })
}

pub(crate) fn binding_value(binding: &SourceBinding) -> Value {
    json!({
        "snapshot": snapshot_value(&binding.snapshot),
        "worktree_fingerprint": binding.worktree_fingerprint,
    })
}

pub(crate) fn execution_source(inventory: &Inventory) -> Result<Value> {
    let mut value = native_json::object(&binding_value(&inventory.source), "完整来源快照")?.clone();
    value.insert("fingerprints".into(), fingerprints(&inventory.files)?);
    Ok(Value::Object(value))
}

pub(crate) fn fingerprints(files: &[FileRecord]) -> Result<Value> {
    let mut groups = ["product", "test_tools", "support"]
        .into_iter()
        .map(|name| (name, Vec::new()))
        .collect::<BTreeMap<_, Vec<Value>>>();
    let mut previous = "";
    let sha256 = Regex::new(r"^[a-f0-9]{64}$")?;
    for file in files {
        if !sha256.is_match(&file.sha256) || file.path.as_str() <= previous {
            return Err("源码清单必须是无重复的排序文件摘要".into());
        }
        previous = &file.path;
        groups
            .get_mut(domains::source_domain(&file.path)?)
            .expect("来源分组固定存在")
            .push(file_value(file));
    }
    let mut result = Map::new();
    for (name, files) in groups {
        result.insert(
            name.into(),
            json!({
                "sha256": digest::plan_hash(&Value::Array(files.clone()))?,
                "files": files.len(),
            }),
        );
    }
    Ok(Value::Object(result))
}

pub(crate) fn backend_domains(inventory: &Inventory) -> Result<Value> {
    let (files, _) = file_inventory_for(&inventory.files)?;
    let mut api = Vec::new();
    let mut worker = Vec::new();
    let mut tools = Vec::new();
    for file in &inventory.files {
        for &role in domains::backend_product_roles(&file.path)? {
            match role {
                "api" => api.push(file.clone()),
                "worker" => worker.push(file.clone()),
                _ => return Err("后端产品角色无效".into()),
            }
        }
        if domains::tool_source(&file.path) {
            tools.push(file.clone());
        }
    }
    Ok(json!({
        "product": {
            "api": source_group(&files, &api)?,
            "worker": source_group(&files, &worker)?,
        },
        "tools": source_group(&files, &tools)?,
        "full": inventory_value(inventory),
    }))
}

pub(crate) fn validate_backend_domains(value: &Value, inventory: &Inventory) -> Result<()> {
    if value != &backend_domains(inventory)? {
        return Err("构建来源分域与完整来源清单不一致".into());
    }
    Ok(())
}

fn snapshot(root: &Root) -> Result<SourceSnapshot> {
    let head = text_output(git(root.path(), &["rev-parse", "HEAD"])?)?;
    if !Regex::new(r"^[a-f0-9]{40}$")?.is_match(&head) {
        return Err("源码 HEAD 不是有效 SHA".into());
    }
    let patch = git(root.path(), &["diff", "--binary", "HEAD"])?;
    let mut files = Vec::new();
    for raw in nul_paths(git(
        root.path(),
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?)? {
        let path = String::from_utf8(raw).map_err(|_| "源码路径必须为 UTF-8")?;
        let file = source_file(root, &path, false)?.ok_or("未跟踪源码文件不存在")?;
        files.push(FileRecord {
            path,
            sha256: digest::file(&file)?.sha256,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let clean = git(
        root.path(),
        &["status", "--porcelain", "--untracked-files=all"],
    )?
    .is_empty();
    Ok(SourceSnapshot {
        head,
        patch_sha256: hex::encode(Sha256::digest(&patch)),
        files,
        clean,
    })
}

fn worktree_fingerprint(root: &Root, commit: &str) -> Result<String> {
    let mut digest = Sha256::new();
    digest_bytes(&mut digest, commit.as_bytes());
    digest_bytes(
        &mut digest,
        &git(
            root.path(),
            &["diff", "--binary", "--no-ext-diff", "HEAD", "--", "."],
        )?,
    );
    for raw in nul_paths(git(
        root.path(),
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?)? {
        let relative = String::from_utf8(raw.clone()).map_err(|_| "源码路径必须为 UTF-8")?;
        let path = source_file(root, &relative, false)?.ok_or("未跟踪源码文件不存在")?;
        digest_bytes(&mut digest, &raw);
        crate::recovery::runtime::dataset_authority::reads::file(&path)?;
        digest_bytes(&mut digest, &fs::read(path)?);
    }
    Ok(format!("sha256:{}", hex::encode(digest.finalize())))
}

fn file_inventory(root: &Root) -> Result<(Vec<FileRecord>, SourceGuard)> {
    let mut paths = nul_paths(git(
        root.path(),
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?)?;
    paths.sort();
    paths.dedup();
    let entries = source_scan::ordered(&paths, !reads::is_active(), |raw| {
        let relative = std::str::from_utf8(raw).map_err(|_| "源码路径必须为 UTF-8")?;
        let Some(path) = source_file(root, relative, true)? else {
            return Ok(None);
        };
        let metadata = fs::symlink_metadata(&path)?;
        let file = FileRecord {
            path: relative.to_owned(),
            sha256: digest::file(&path)?.sha256,
        };
        let mode = json!({
            "path": relative,
            "executable": executable(&metadata),
        });
        Ok(Some((file, mode)))
    })?;
    let mut files = Vec::new();
    let mut modes = Vec::new();
    for (file, mode) in entries.into_iter().flatten() {
        files.push(file);
        modes.push(mode);
    }
    let head = text_output(git(root.path(), &["rev-parse", "HEAD"])?)?;
    let guard = SourceGuard {
        head,
        index_sha256: hex::encode(Sha256::digest(git(
            root.path(),
            &["ls-files", "--stage", "-z"],
        )?)),
        modes_sha256: digest::plan_hash(&Value::Array(modes))?,
    };
    Ok((files, guard))
}

fn file_inventory_for(files: &[FileRecord]) -> Result<(BTreeMap<String, FileRecord>, ())> {
    let mut selected = BTreeMap::new();
    for file in files {
        domains::source_domain(&file.path)?;
        if selected.insert(file.path.clone(), file.clone()).is_some() {
            return Err("源码清单必须是无重复的排序文件摘要".into());
        }
    }
    Ok((selected, ()))
}

fn source_group(selected: &BTreeMap<String, FileRecord>, files: &[FileRecord]) -> Result<Value> {
    let values = files
        .iter()
        .map(|file| {
            selected
                .get(&file.path)
                .map(file_value)
                .ok_or_else(|| "构建来源文件不在完整来源清单中".into())
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "sha256": digest::plan_hash(&Value::Array(values))?,
        "files": files.iter().map(|file| file.path.clone()).collect::<Vec<_>>(),
    }))
}

fn source_file(root: &Root, relative: &str, allow_missing: bool) -> Result<Option<PathBuf>> {
    domains::source_domain(relative)?;
    let path = Path::new(relative);
    let mut current = root.path().to_path_buf();
    let mut metadata = None;
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(value) => {
                if digest::linked(&value) {
                    return Err("源码路径不能包含符号链接或 junction".into());
                }
                metadata = Some(value);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_missing => {
                return Ok(None);
            }
            Err(error) => {
                return Err(format!("无法读取源码文件 {}：{error}", current.display()).into());
            }
        }
    }
    if metadata.is_none_or(|metadata| !metadata.is_file()) {
        return Err("源码清单只能绑定普通文件".into());
    }
    let canonical = fs::canonicalize(&current)?;
    if !canonical.starts_with(root.canonical()) {
        return Err("源码文件越界".into());
    }
    Ok(Some(current))
}

fn parse_files(value: &Value, label: &str) -> Result<Vec<FileRecord>> {
    let mut previous = String::new();
    let mut result = Vec::new();
    for item in native_json::array(value, label)? {
        let object = native_json::exact(item, &["path", "sha256"], label)?;
        let path = native_json::text(object.get("path").unwrap(), "源码路径")?.to_owned();
        let sha256 = digest_value(object.get("sha256").unwrap(), 64, "源码文件摘要")?;
        if path <= previous {
            return Err(format!("{label}必须是无重复的排序文件摘要").into());
        }
        domains::source_domain(&path)?;
        previous = path.clone();
        result.push(FileRecord { path, sha256 });
    }
    Ok(result)
}

fn digest_value(value: &Value, size: usize, label: &str) -> Result<String> {
    let value = native_json::text(value, label)?;
    if value.len() != size
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{label}格式无效").into());
    }
    Ok(value.to_owned())
}

fn snapshot_value(snapshot: &SourceSnapshot) -> Value {
    json!({
        "head": snapshot.head,
        "patch_sha256": snapshot.patch_sha256,
        "files": files_value(&snapshot.files),
        "clean": snapshot.clean,
    })
}

fn files_value(files: &[FileRecord]) -> Value {
    Value::Array(files.iter().map(file_value).collect())
}

fn file_value(file: &FileRecord) -> Value {
    json!({"path": file.path, "sha256": file.sha256})
}

fn git(root: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(arguments);
    // 来源仅由显式仓库决定，拒绝继承的仓库、索引、配置和 replace 覆盖。
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let output = command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .output()
        .map_err(|error| format!("无法执行 Git 源码观察：{error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Git 源码观察失败（{}）：{}", output.status, detail.trim()).into());
    }
    Ok(output.stdout)
}

fn nul_paths(output: Vec<u8>) -> Result<Vec<Vec<u8>>> {
    Ok(output
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn text_output(value: Vec<u8>) -> Result<String> {
    String::from_utf8(value)
        .map(|value| value.trim().to_owned())
        .map_err(|_| "Git 源码观察输出必须为 UTF-8".into())
}

fn digest_bytes(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}

#[cfg(unix)]
fn executable(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111
}

#[cfg(not(unix))]
fn executable(_metadata: &fs::Metadata) -> u32 {
    0
}
