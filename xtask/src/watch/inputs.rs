use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use super::{WatchInputRoots, relevant_paths};
use crate::Result;

#[derive(Clone)]
pub(super) struct Mapping {
    pub(super) root: PathBuf,
    pub(super) inputs: Option<WatchInputRoots>,
}

impl Mapping {
    pub(super) fn new(root: &Path, inputs: Option<WatchInputRoots>) -> Result<Self> {
        let root = ordinary_directory(root, "workspace")?;
        let inputs = inputs.map(|inputs| -> Result<WatchInputRoots> {
            let config_dir = ordinary_directory(&inputs.config_dir, "config 输入")?;
            let locales_dir = ordinary_directory(&inputs.locales_dir, "locales 输入")?;
            if config_dir.starts_with(&locales_dir) || locales_dir.starts_with(&config_dir) {
                return Err(format!(
                    "登记输入失败：config {} 与 locales {} 相同或彼此包含；请指定两个互不包含的普通目录",
                    config_dir.display(), locales_dir.display(),
                ).into());
            }
            Ok(WatchInputRoots { config_dir, locales_dir })
        }).transpose()?;
        Ok(Self { root, inputs })
    }

    pub(super) fn paths(&self, paths: &[PathBuf]) -> BTreeSet<String> {
        paths
            .iter()
            .flat_map(|path| {
                if let Some(inputs) = &self.inputs {
                    for (directory, prefix) in [
                        (&inputs.config_dir, "config"),
                        (&inputs.locales_dir, "locales"),
                    ] {
                        if let Ok(relative) = path.strip_prefix(directory) {
                            let relative = if relative.as_os_str().is_empty() {
                                "__root__".to_owned()
                            } else {
                                relative.to_string_lossy().replace('\\', "/")
                            };
                            return BTreeSet::from([format!("{prefix}/{relative}")]);
                        }
                    }
                }
                if path == &self.root.join(".local-tests/tools.toml") {
                    return BTreeSet::from(["config/tools.local.toml".to_owned()]);
                }
                relevant_paths(&self.root, std::slice::from_ref(path))
            })
            .collect()
    }

    pub(super) fn changed_paths(&self, next: &Self) -> BTreeSet<String> {
        let mut paths = BTreeSet::new();
        if self.inputs.as_ref().map(|inputs| &inputs.config_dir)
            != next.inputs.as_ref().map(|inputs| &inputs.config_dir)
        {
            paths.insert("config/__root__".into());
        }
        if self.inputs.as_ref().map(|inputs| &inputs.locales_dir)
            != next.inputs.as_ref().map(|inputs| &inputs.locales_dir)
        {
            paths.insert("locales/__root__".into());
        }
        paths
    }
}

pub(super) fn ordinary_directory(path: &Path, category: &str) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(format!("登记 {category} 失败：路径为空；请指定已有普通目录").into());
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| io_error("解析当前目录", path, &error))?
            .join(path)
    };
    if !absolute.is_absolute() {
        return Err(format!(
            "登记 {category} {} 失败：不是完整绝对路径；请指定明确的绝对目录",
            path.display()
        )
        .into());
    }
    let mut checked = PathBuf::new();
    for component in absolute.components() {
        if component == Component::CurDir {
            continue;
        }
        checked.push(component);
        // Windows 盘符前缀尚不是可查询的完整根路径，等待 RootDir 分量。
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        if !checked.has_root() {
            continue;
        }
        let metadata = fs::symlink_metadata(&checked).map_err(|error| {
            io_error(
                &format!("检查 {category} {} 的祖先", path.display()),
                &checked,
                &error,
            )
        })?;
        if linked(&metadata) || !metadata.is_dir() {
            return Err(format!(
                "登记 {category} {} 失败：{} 是链接、重解析点或非目录；请改用无链接祖先的普通目录",
                path.display(),
                checked.display(),
            )
            .into());
        }
        if component == Component::ParentDir {
            checked.pop();
            checked.pop();
        }
    }
    let metadata = fs::symlink_metadata(&checked)
        .map_err(|error| io_error(&format!("检查 {category} 最终目录"), &checked, &error))?;
    if linked(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "登记 {category} {} 失败：最终路径不是无链接的普通目录；请修正目录类型",
            checked.display()
        )
        .into());
    }
    checked
        .canonicalize()
        .map_err(|error| io_error("规范化已检查的输入目录", &checked, &error).into())
}

pub(super) fn io_error(stage: &str, path: &Path, error: &std::io::Error) -> String {
    format!(
        "{stage}失败：路径 {}，error.kind={:?}，raw_os_error={:?}；请创建普通目录并检查路径和访问权限",
        path.display(),
        error.kind(),
        error.raw_os_error()
    )
}

#[cfg(windows)]
fn linked(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn linked(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
