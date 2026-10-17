use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::digest;
use crate::Result;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileRecord {
    path: String,
    sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceSnapshot {
    head: String,
    patch_sha256: String,
    files: Vec<FileRecord>,
    clean: bool,
}

struct Inventory {
    snapshot: SourceSnapshot,
    worktree_fingerprint: String,
    files: Vec<FileRecord>,
}

pub(crate) fn current(root: &Path) -> Result<Value> {
    let inventory = capture(root)?;
    Ok(json!({
        "snapshot": snapshot_value(&inventory.snapshot),
        "worktree_fingerprint": inventory.worktree_fingerprint,
        "fingerprints": fingerprints(&inventory.files)?,
    }))
}

fn capture(root: &Path) -> Result<Inventory> {
    let before = snapshot(root)?;
    let fingerprint = worktree_fingerprint(root, &before.head)?;
    let (files, guard) = file_inventory(root)?;
    if snapshot(root)? != before
        || worktree_fingerprint(root, &before.head)? != fingerprint
        || guard.get("head").and_then(Value::as_str) != Some(before.head.as_str())
    {
        return Err("采集指纹期间完整源码发生变化".into());
    }
    Ok(Inventory {
        snapshot: before,
        worktree_fingerprint: fingerprint,
        files,
    })
}

fn snapshot(root: &Path) -> Result<SourceSnapshot> {
    let head = text_output(git(root, &["rev-parse", "HEAD"])?)?;
    if !Regex::new(r"^[a-f0-9]{40}$")?.is_match(&head) {
        return Err("源码 HEAD 不是有效 SHA".into());
    }
    let patch = git(root, &["diff", "--binary", "HEAD"])?;
    let mut files = Vec::new();
    for raw in nul_paths(git(
        root,
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
    let clean = git(root, &["status", "--porcelain", "--untracked-files=all"])?.is_empty();
    Ok(SourceSnapshot {
        head,
        patch_sha256: hex::encode(Sha256::digest(&patch)),
        files,
        clean,
    })
}

fn worktree_fingerprint(root: &Path, commit: &str) -> Result<String> {
    let mut digest = Sha256::new();
    digest_bytes(&mut digest, commit.as_bytes());
    digest_bytes(
        &mut digest,
        &git(
            root,
            &["diff", "--binary", "--no-ext-diff", "HEAD", "--", "."],
        )?,
    );
    for raw in nul_paths(git(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?)? {
        let relative = String::from_utf8(raw.clone()).map_err(|_| "源码路径必须为 UTF-8")?;
        let path = source_file(root, &relative, false)?.ok_or("未跟踪源码文件不存在")?;
        digest_bytes(&mut digest, &raw);
        digest_bytes(&mut digest, &fs::read(path)?);
    }
    Ok(format!("sha256:{}", hex::encode(digest.finalize())))
}

fn file_inventory(root: &Path) -> Result<(Vec<FileRecord>, Value)> {
    let mut paths = nul_paths(git(
        root,
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
    let mut files = Vec::new();
    let mut modes = Vec::new();
    for raw in paths {
        let relative = String::from_utf8(raw).map_err(|_| "源码路径必须为 UTF-8")?;
        let Some(path) = source_file(root, &relative, true)? else {
            continue;
        };
        let metadata = fs::symlink_metadata(&path)?;
        files.push(FileRecord {
            path: relative.clone(),
            sha256: digest::file(&path)?.sha256,
        });
        modes.push(json!({
            "path": relative,
            "executable": executable(&metadata),
        }));
    }
    let head = text_output(git(root, &["rev-parse", "HEAD"])?)?;
    let index = git(root, &["ls-files", "--stage", "-z"])?;
    let guard = json!({
        "head": head,
        "index_sha256": hex::encode(Sha256::digest(index)),
        "modes_sha256": digest::plan_hash(&Value::Array(modes))?,
    });
    Ok((files, guard))
}

fn source_file(root: &Path, relative: &str, allow_missing: bool) -> Result<Option<PathBuf>> {
    source_domain(relative)?;
    let path = Path::new(relative);
    let mut current = root.to_path_buf();
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
    let canonical_root = fs::canonicalize(root)?;
    let canonical = fs::canonicalize(&current)?;
    if !canonical.starts_with(canonical_root) {
        return Err("源码文件越界".into());
    }
    Ok(Some(current))
}

fn fingerprints(files: &[FileRecord]) -> Result<Value> {
    let mut groups = ["product", "test_tools", "support"]
        .into_iter()
        .map(|name| (name.to_owned(), Vec::new()))
        .collect::<BTreeMap<_, Vec<Value>>>();
    let mut previous = "";
    let sha256 = Regex::new(r"^[a-f0-9]{64}$")?;
    for file in files {
        if !sha256.is_match(&file.sha256) || file.path.as_str() <= previous {
            return Err("源码清单必须是无重复的排序文件摘要".into());
        }
        previous = &file.path;
        groups
            .get_mut(source_domain(&file.path)?)
            .unwrap()
            .push(json!({"path": file.path, "sha256": file.sha256}));
    }
    let mut result = serde_json::Map::new();
    for (name, files) in groups {
        result.insert(
            name,
            json!({
                "sha256": digest::plan_hash(&Value::Array(files.clone()))?,
                "files": files.len(),
            }),
        );
    }
    Ok(Value::Object(result))
}

fn source_domain(relative: &str) -> Result<&'static str> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || relative.contains('\\')
        || relative != path.to_string_lossy()
    {
        return Err("源码清单路径无效".into());
    }
    let parts = relative.split('/').collect::<Vec<_>>();
    if parts.first() == Some(&"tests")
        || parts.starts_with(&["xtask", "tests"])
        || parts.len() >= 3 && parts[0] == "crates" && parts[2] == "tests"
    {
        return Ok("test_tools");
    }
    if parts.first() == Some(&"tools")
        && matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("py" | "mjs" | "js" | "txt" | "toml" | "json")
        )
    {
        return Ok("test_tools");
    }
    if parts.first() == Some(&".github") {
        return Ok("test_tools");
    }
    if parts.first() == Some(&"docs")
        || matches!(relative, "README.md" | "CHANGELOG.md" | "LICENSE")
    {
        return Ok("support");
    }
    Ok("product")
}

fn snapshot_value(snapshot: &SourceSnapshot) -> Value {
    json!({
        "head": snapshot.head,
        "patch_sha256": snapshot.patch_sha256,
        "files": snapshot.files.iter().map(|file| json!({
            "path": file.path,
            "sha256": file.sha256,
        })).collect::<Vec<_>>(),
        "clean": snapshot.clean,
    })
}

fn git(root: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
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
fn executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_metadata: &fs::Metadata) -> bool {
    false
}
