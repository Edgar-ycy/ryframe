use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

type Files = BTreeMap<PathBuf, Option<Vec<u8>>>;

/// 候选编译输入与正式文件的完整前像；不把缓存、密钥目录或链接当成源码。
pub(super) struct SourceSnapshot {
    pub(super) root: PathBuf,
    files: Files,
}

#[derive(Debug)]
pub(super) struct FileChange {
    pub(super) relative: PathBuf,
    pub(super) before: Option<Vec<u8>>,
    pub(super) after: Option<Vec<u8>>,
}

impl SourceSnapshot {
    pub(super) fn input_paths(&self) -> impl Iterator<Item = &Path> {
        self.files.keys().map(PathBuf::as_path)
    }
    pub(super) fn capture(root: &Path) -> Result<Self, String> {
        let root = fs::canonicalize(root)
            .map_err(|error| format!("无法解析源码目录 {}：{error}", root.display()))?;
        let paths = source_paths(&root)?;
        let mut files = BTreeMap::new();
        for path in paths {
            files.insert(path.clone(), read_optional(&safe_path(&root, &path)?)?);
        }
        // 候选标记可以被 ignore，但属于契约事务，仍须核对存在与不存在的前像。
        let marker = PathBuf::from("openapi/candidate.json");
        files.insert(marker.clone(), read_optional(&safe_path(&root, &marker)?)?);
        Ok(Self { root, files })
    }

    pub(super) fn copy_to(&self, candidate: &Path) -> Result<(), String> {
        if candidate.exists() {
            return Err(format!("候选目录已存在：{}；不会覆盖", candidate.display()));
        }
        fs::create_dir_all(candidate).map_err(|error| format!("无法创建候选目录：{error}"))?;
        for (relative, bytes) in &self.files {
            if let Some(bytes) = bytes {
                let path = safe_path(candidate, relative)?;
                fs::create_dir_all(path.parent().ok_or("候选文件缺少父目录")?).map_err(io_error)?;
                fs::write(&path, bytes).map_err(io_error)?;
            }
        }
        Ok(())
    }

    pub(super) fn verify(&self) -> Result<(), String> {
        let current = Self::capture(&self.root)?;
        if self.files != current.files {
            let path = self
                .files
                .keys()
                .chain(current.files.keys())
                .find(|path| self.files.get(*path) != current.files.get(*path));
            return Err(format!(
                "源码在候选生成期间发生变化：{}；本批未安装，保留并发修改后重新预览",
                path.map(|path| self.root.join(path))
                    .unwrap_or_else(|| self.root.clone())
                    .display()
            ));
        }
        Ok(())
    }

    pub(super) fn changes(&self, candidate: &Path) -> Result<Vec<FileChange>, String> {
        let after = Self::capture(candidate)?;
        let paths = self
            .files
            .keys()
            .chain(after.files.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        Ok(paths
            .into_iter()
            .filter_map(|relative| {
                let before = self.files.get(&relative).cloned().flatten();
                let after = after.files.get(&relative).cloned().flatten();
                (before != after).then_some(FileChange {
                    relative,
                    before,
                    after,
                })
            })
            .collect())
    }
}

fn source_paths(root: &Path) -> Result<BTreeSet<PathBuf>, String> {
    if root.join(".git").exists() {
        let top = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(root)
            .output()
            .map_err(io_error)?;
        if !top.status.success() {
            return Err("无法核对源码 Git 根目录；未创建候选".into());
        }
        let top = String::from_utf8(top.stdout).map_err(|_| "Git 根目录不是 UTF-8")?;
        if fs::canonicalize(top.trim()).map_err(io_error)? != root {
            return Err("源码目录与 Git 根目录不同；请选择独立仓库根目录".into());
        }
        let output = Command::new("git")
            .args([
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
                "--",
            ])
            .current_dir(root)
            .output()
            .map_err(io_error)?;
        if !output.status.success() {
            return Err(format!(
                "读取源码文件清单失败（{}）：{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        return output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| {
                String::from_utf8(path.to_vec())
                    .map(PathBuf::from)
                    .map_err(|_| "源码路径不是 UTF-8；未创建候选".to_owned())
            })
            .filter(|path| path.as_ref().map_or(true, |path| !excluded(path)))
            .collect();
    }
    let mut paths = BTreeSet::new();
    collect_directory(root, Path::new(""), &mut paths)?;
    Ok(paths)
}

fn collect_directory(
    root: &Path,
    relative: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    for entry in fs::read_dir(root.join(relative)).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = relative.join(entry.file_name());
        if excluded(&path) {
            continue;
        }
        let absolute = safe_path(root, &path)?;
        let metadata = fs::symlink_metadata(&absolute).map_err(io_error)?;
        if metadata.is_dir() {
            collect_directory(root, &path, paths)?;
        } else if metadata.is_file() {
            paths.insert(path);
        } else {
            return Err(format!("候选输入不是普通文件：{}", absolute.display()));
        }
    }
    Ok(())
}

fn excluded(path: &Path) -> bool {
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        let name = name.to_string_lossy();
        matches!(
            name.as_ref(),
            ".git" | "target" | "node_modules" | ".local-tests" | ".aws" | ".codex" | ".agents"
        ) || name.starts_with(".ryframe-stage-")
            || name == ".xtask-contract.lock"
    })
}

pub(super) fn safe_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative.as_os_str().is_empty() {
        return Err("源码文件路径不能为空".into());
    }
    let mut target = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(format!("源码路径越界：{}", relative.display()));
        };
        target.push(name);
        match fs::symlink_metadata(&target) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(format!(
                    "源码路径包含链接或重解析点：{}；未创建候选",
                    target.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(target)
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("无法读取源码前像 {}：{error}", path.display())),
    }
}

fn io_error(error: std::io::Error) -> String {
    format!("候选工作区文件操作失败：{error}；本批未安装")
}
