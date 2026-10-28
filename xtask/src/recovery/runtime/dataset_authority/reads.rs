//! 本 producer 的读取窗口；原生读取端口只登记实际消费的文件，不猜测 JSON 中的路径。
use crate::{Result, recovery::monitoring::execution::ProtectedInputs};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

thread_local! {
    static ACTIVE: RefCell<Option<ReadSet>> = const { RefCell::new(None) };
    // with_set 临时取出集合期间也必须保持同线程读取，直到 Window 释放。
    static WINDOW_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn is_active() -> bool {
    WINDOW_ACTIVE.get()
}

#[derive(Default)]
struct ReadSet {
    files: BTreeMap<PathBuf, ProtectedInputs>,
    directories: BTreeMap<PathBuf, Directory>,
    absent: BTreeSet<PathBuf>,
    sources: BTreeMap<PathBuf, crate::recovery::clone::native::source::Inventory>,
}

#[derive(PartialEq, Eq)]
struct Directory {
    names: BTreeSet<std::ffi::OsString>,
    identity: (u64, u64),
}

pub(super) struct Window {
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl Window {
    pub(super) fn begin() -> Result<Self> {
        ACTIVE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if is_active() {
                return Err("恢复静态读取窗口不能嵌套".into());
            }
            *slot = Some(ReadSet::default());
            WINDOW_ACTIVE.set(true);
            Ok(Self {
                _thread_bound: std::marker::PhantomData,
            })
        })
    }

    pub(super) fn finish<T>(&self, result: Result<T>) -> Result<T> {
        let checked = with_set(|set| {
            for (root, expected) in &set.sources {
                if crate::recovery::clone::native::source::capture(root)? != *expected {
                    return Err("恢复静态核验期间 Git 来源、索引或完整文件集合漂移".into());
                }
            }
            for guard in set.files.values() {
                guard.assert_unchanged()?;
            }
            for (path, expected) in &set.directories {
                if names(path)? != *expected {
                    return Err("恢复静态核验期间输入目录成员漂移".into());
                }
            }
            for path in &set.absent {
                if path.try_exists()? {
                    return Err("恢复静态核验期间原缺省配置出现".into());
                }
            }
            Ok(())
        });
        match (result, checked) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(after)) => {
                Err(format!("{error}；输入集合后验同时失败：{after}").into())
            }
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        ACTIVE.with(|slot| {
            slot.borrow_mut().take();
        });
        WINDOW_ACTIVE.set(false);
    }
}

// 取出集合后再打开保护句柄，避免保护器自身计算摘要时再次登记。
fn with_set(action: impl FnOnce(&mut ReadSet) -> Result<()>) -> Result<()> {
    ACTIVE.with(|slot| {
        let Some(mut set) = slot.borrow_mut().take() else {
            return Ok(());
        };
        let result = action(&mut set);
        *slot.borrow_mut() = Some(set);
        result
    })
}

pub(crate) fn file(path: &Path) -> Result<()> {
    with_set(|set| record_file(set, path))
}

fn record_file(set: &mut ReadSet, path: &Path) -> Result<()> {
    let path = std::path::absolute(path)?;
    if let std::collections::btree_map::Entry::Vacant(entry) = set.files.entry(path.clone()) {
        entry.insert(ProtectedInputs::open([path.clone()])?);
    }
    if let Some(parent) = path.parent() {
        remember_directory(set, parent)?;
    }
    Ok(())
}

pub(crate) fn directory(path: &Path) -> Result<()> {
    with_set(|set| remember_directory(set, path))
}

pub(crate) fn entries(path: impl AsRef<Path>) -> Result<std::fs::ReadDir> {
    directory(path.as_ref())?;
    Ok(std::fs::read_dir(path)?)
}

pub(crate) fn source(
    root: &Path,
    value: &crate::recovery::clone::native::source::Inventory,
) -> Result<()> {
    with_set(|set| {
        for file in &value.files {
            let path = std::path::absolute(root.join(&file.path))?;
            if !set.files.contains_key(&path) {
                return Err(
                    "来源文件读取未登记到当前静态权威窗口；并行来源读取必须显式传递保护上下文"
                        .into(),
                );
            }
        }
        if let Some(previous) = set.sources.get(root) {
            if previous != value {
                return Err("恢复静态核验期间完整来源漂移".into());
            }
        } else {
            set.sources.insert(root.to_owned(), value.clone());
        }
        Ok(())
    })
}

pub(crate) fn optional(path: &Path) -> Result<()> {
    with_set(|set| {
        if path.try_exists()? {
            record_file(set, path)
        } else {
            set.absent.insert(path.to_owned());
            Ok(())
        }
    })
}

pub(crate) fn tool_settings(backend: &Path) -> Result<()> {
    for relative in ["config/tools.toml", ".local-tests/tools.toml"] {
        optional(&backend.join(relative))?;
    }
    Ok(())
}

fn remember_directory(set: &mut ReadSet, path: &Path) -> Result<()> {
    if let std::collections::btree_map::Entry::Vacant(entry) =
        set.directories.entry(path.to_owned())
    {
        entry.insert(names(path)?);
    }
    Ok(())
}

fn names(path: &Path) -> Result<Directory> {
    if crate::recovery::clone::native::digest::linked_path(path)?.contains(&true) {
        return Err("恢复静态输入目录经过链接".into());
    }
    let metadata = std::fs::symlink_metadata(path)?;
    let identity =
        crate::recovery::clone::native::resources::common::device_inode_for(path, &metadata)?;
    let names = std::fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<_>>()?;
    let after = std::fs::symlink_metadata(path)?;
    if crate::recovery::clone::native::resources::common::device_inode_for(path, &after)?
        != identity
    {
        return Err("恢复静态输入目录读取期间被替换".into());
    }
    Ok(Directory { names, identity })
}
