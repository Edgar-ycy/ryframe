use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, DirEntry, FileType},
    io::ErrorKind,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    Result,
    recovery::clone::native::{digest, resources::common, target_verify::guard::Inputs},
};

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static CAPTURE: RefCell<Option<InputsRead>> = const { RefCell::new(None) };
}

pub(super) fn is_active() -> bool {
    ACTIVE.get()
}

#[derive(Default)]
struct InputsRead {
    files: BTreeMap<PathBuf, Inputs>,
    directories: BTreeMap<PathBuf, Inputs>,
    members: BTreeMap<PathBuf, BTreeMap<OsString, Node>>,
    exists: BTreeMap<PathBuf, Option<Node>>,
    modified: BTreeMap<PathBuf, SystemTime>,
}

#[derive(PartialEq, Eq)]
struct Node {
    kind: FileType,
    linked: bool,
}

pub(in crate::recovery) struct Fence {
    inputs: InputsRead,
    current_dir: PathBuf,
    workspace: PathBuf,
}

impl Fence {
    pub(in crate::recovery) fn capture<T>(
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<(T, Self)> {
        if is_active() || super::super::is_active() {
            return Err("prepare 存储历史读取窗口不能嵌套".into());
        }
        let current_dir = std::env::current_dir()?;
        let workspace = crate::workspace::root_dir();
        let mut initial = InputsRead::default();
        initial.hold_directory(&current_dir)?;
        initial.hold_directory(&workspace)?;
        CAPTURE.with(|slot| *slot.borrow_mut() = Some(initial));
        ACTIVE.set(true);
        let _reset = Reset;
        let result = operation();
        let inputs = CAPTURE
            .with(|slot| slot.borrow_mut().take())
            .ok_or("prepare 存储历史丢失同次读取证据")?;
        let fence = Self {
            inputs,
            current_dir,
            workspace,
        };
        Ok((result?, fence))
    }

    pub(in crate::recovery) fn check(&self) -> Result<()> {
        self.check_with(Inputs::check)
    }

    pub(in crate::recovery) fn checkpoint(&self) -> Result<()> {
        self.check_with(Inputs::check_identities)
    }

    fn check_with(&self, check_input: fn(&Inputs) -> Result<()>) -> Result<()> {
        if super::super::is_active()
            || is_active()
            || std::env::current_dir()? != self.current_dir
            || crate::workspace::root_dir() != self.workspace
        {
            return Err("prepare 存储历史的协调工作目录发生变化".into());
        }
        for guard in self
            .inputs
            .directories
            .values()
            .chain(self.inputs.files.values())
        {
            check_input(guard)?;
        }
        for (path, expected) in &self.inputs.members {
            if &members(&fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?)? != expected {
                return Err("prepare 存储历史目录成员或节点类型发生变化".into());
            }
        }
        for (path, expected) in &self.inputs.exists {
            if &node(path)? != expected {
                return Err("prepare 存储历史可选路径存在性或节点类型发生变化".into());
            }
        }
        for (path, expected) in &self.inputs.modified {
            if fs::metadata(path)?.modified()? != *expected {
                return Err("prepare 存储历史收据的修改时间发生变化".into());
            }
        }
        Ok(())
    }
}

struct Reset;

impl Drop for Reset {
    fn drop(&mut self) {
        CAPTURE.with(|slot| slot.borrow_mut().take());
        ACTIVE.set(false);
    }
}

impl InputsRead {
    fn hold_file(&mut self, path: &Path) -> Result<()> {
        let path = std::path::absolute(path)?;
        if let Some(parent) = path.parent() {
            self.hold_directory(parent)?;
        }
        if let std::collections::btree_map::Entry::Vacant(entry) = self.files.entry(path.clone()) {
            entry.insert(Inputs::open([path])?);
        }
        Ok(())
    }

    fn hold_directory(&mut self, path: &Path) -> Result<()> {
        let path = std::path::absolute(path)?;
        if let std::collections::btree_map::Entry::Vacant(entry) =
            self.directories.entry(path.clone())
        {
            entry.insert(Inputs::directory(&path)?);
        }
        Ok(())
    }
}

// 原文件保护打开期间也使用 digest::open；临时移出证据集合避免再次保护同一文件。
fn capture<T>(read: impl FnOnce(&mut InputsRead) -> Result<T>) -> Result<Option<T>> {
    CAPTURE.with(|slot| {
        let Some(mut inputs) = slot.borrow_mut().take() else {
            return Ok(None);
        };
        let result = read(&mut inputs);
        *slot.borrow_mut() = Some(inputs);
        result.map(Some)
    })
}

pub(in crate::recovery) fn file(path: &Path) -> Result<()> {
    capture(|inputs| inputs.hold_file(path)).map(|_| ())
}

pub(super) fn directory(path: &Path) -> Result<bool> {
    Ok(capture(|inputs| inputs.hold_directory(path))?.is_some())
}

pub(super) fn entries(path: &Path) -> Result<Option<Vec<DirEntry>>> {
    capture(|inputs| {
        let path = std::path::absolute(path)?;
        inputs.hold_directory(&path)?;
        let entries = fs::read_dir(&path)?.collect::<std::io::Result<Vec<_>>>()?;
        let values = members(&entries)?;
        remember(&mut inputs.members, path, values)?;
        Ok(entries)
    })
}

pub(super) fn exists(path: &Path) -> Result<Option<bool>> {
    capture(|inputs| {
        let path = std::path::absolute(path)?;
        inputs.hold_directory(path.parent().ok_or("可选证据缺少父目录")?)?;
        let current = node(&path)?;
        if current.as_ref().is_some_and(|value| value.linked) {
            return Err("prepare 存储历史的可选证据不能经过链接".into());
        }
        let exists = current.is_some();
        remember(&mut inputs.exists, path, current)?;
        Ok(exists)
    })
}

pub(super) fn optional_file(path: &Path) -> Result<Option<()>> {
    capture(|inputs| {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("prepare 存储历史的可选文件缺少普通文件名")?;
        if filename.contains(':') || filename.ends_with([' ', '.']) {
            return Err("prepare 存储历史的可选文件不能使用路径别名".into());
        }
        let path = std::path::absolute(path)?;
        inputs.hold_directory(path.parent().ok_or("可选文件缺少父目录")?)?;
        let state = node(&path)?;
        if let Some(file) = &state {
            if !file.kind.is_file() || file.linked {
                return Err("prepare 存储历史的可选输入必须是普通文件".into());
            }
            inputs.hold_file(&path)?;
            let metadata = fs::symlink_metadata(&path)?;
            if common::link_count_for(&path, &metadata)? != 1 {
                return Err("prepare 存储历史的可选文件不能使用硬链接别名".into());
            }
        }
        remember(&mut inputs.exists, path, state)
    })
}

pub(super) fn modified(path: &Path) -> Result<Option<SystemTime>> {
    capture(|inputs| {
        let path = std::path::absolute(path)?;
        inputs.hold_file(&path)?;
        let modified = fs::metadata(&path)?.modified()?;
        remember(&mut inputs.modified, path, modified)?;
        Ok(modified)
    })
}

fn remember<T: Eq>(observed: &mut BTreeMap<PathBuf, T>, path: PathBuf, value: T) -> Result<()> {
    use std::collections::btree_map::Entry;
    match observed.entry(path) {
        Entry::Occupied(previous) if previous.get() != &value => {
            Err("prepare 存储历史在首次核验期间发生变化".into())
        }
        Entry::Occupied(_) => Ok(()),
        Entry::Vacant(entry) => {
            entry.insert(value);
            Ok(())
        }
    }
}

fn members(entries: &[DirEntry]) -> Result<BTreeMap<OsString, Node>> {
    entries
        .iter()
        .map(|entry| {
            let value = node(&entry.path())?.ok_or("prepare 存储历史目录成员在枚举后消失")?;
            if value.kind != entry.file_type()? {
                return Err("prepare 存储历史目录成员在枚举后改变类型".into());
            }
            Ok((entry.file_name(), value))
        })
        .collect()
}

fn node(path: &Path) -> Result<Option<Node>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(Node {
            kind: metadata.file_type(),
            linked: digest::linked(&metadata),
        })),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Result<tempfile::TempDir> {
        let root = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&root)?;
        Ok(tempfile::Builder::new()
            .prefix("held-history-")
            .tempdir_in(root)?)
    }

    #[test]
    fn held_history_rejects_new_directory_members_without_holding_log_bytes() -> Result<()> {
        use crate::recovery::runtime::dataset_authority::reads;

        let temporary = fixture()?;
        let receipt = temporary.path().join("receipt.json");
        let log = temporary.path().join("service.log");
        fs::write(&receipt, b"{}")?;
        fs::write(&log, b"first")?;
        let (_, fence) = Fence::capture(|| {
            crate::recovery::clone::native::json::read(&receipt)?;
            let names = reads::entries(temporary.path())?
                .map(|entry| Ok(entry?.file_name()))
                .collect::<Result<Vec<_>>>()?;
            assert_eq!(names.len(), 2);
            assert!(names.iter().any(|name| name == "receipt.json"));
            assert!(names.iter().any(|name| name == "service.log"));
            super::super::modified(&receipt)?;
            Ok(())
        })?;
        fence.check()?;
        fence.checkpoint()?;
        assert!(fs::write(&receipt, b"changed").is_err());
        fs::write(&log, b"second")?;
        fence.checkpoint()?;
        fs::write(temporary.path().join("added"), b"new")?;
        assert!(
            fence
                .checkpoint()
                .unwrap_err()
                .to_string()
                .contains("目录成员")
        );
        fs::remove_file(temporary.path().join("added"))?;
        fence.checkpoint()?;
        fs::remove_file(&log)?;
        fs::create_dir(&log)?;
        assert!(fence.check().unwrap_err().to_string().contains("目录成员"));
        Ok(())
    }

    #[test]
    fn held_history_rejects_a_new_optional_path_and_preserves_outer_window() -> Result<()> {
        use crate::recovery::runtime::dataset_authority::reads;

        let temporary = fixture()?;
        let optional = temporary.path().join("optional.json");
        let present = temporary.path().join("present.json");
        fs::write(&present, b"{}")?;
        let window = super::super::super::Window::direct()?;
        assert!(Fence::capture(|| Ok(())).is_err());
        window.finish(Ok(()))?;
        drop(window);
        assert!(
            Fence::capture(|| reads::optional(&temporary.path().join("present.json."))).is_err()
        );
        let (_, fence) = Fence::capture(|| {
            reads::optional(&optional)?;
            reads::optional(&present)?;
            Ok(())
        })?;
        fence.check()?;
        assert!(fs::write(&present, b"changed").is_err());
        fence.checkpoint()?;
        fs::write(&optional, b"{}")?;
        assert!(
            fence
                .checkpoint()
                .unwrap_err()
                .to_string()
                .contains("可选路径")
        );
        Ok(())
    }
}
