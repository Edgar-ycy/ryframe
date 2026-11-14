//! 持有已登记来源的目录与文件身份。跳过目录的入口仍受保护。
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    path::{Component, Path, PathBuf},
};

use super::{FrozenEntry, Identity, common, digest, path_handle_identity};
use crate::Result;

pub(in crate::recovery) struct HeldSourceTree {
    root: Directory,
}

impl HeldSourceTree {
    pub(in crate::recovery) fn open(
        root: &Path,
        files: impl IntoIterator<Item = PathBuf>,
        skipped: &[&str],
    ) -> Result<Self> {
        let skipped = skip_names(skipped)?;
        let root = FrozenEntry::open_identity(root, true, false)?;
        let mut required = BTreeSet::new();
        for file in files {
            let relative = file.strip_prefix(&root.path)?;
            let mut parts = relative.components();
            let Some(Component::Normal(first)) = parts.next() else {
                return Err("受保护来源文件必须位于登记根下".into());
            };
            if parts.any(|part| !matches!(part, Component::Normal(_)))
                || skipped.contains(&first.to_ascii_lowercase())
            {
                return Err("受保护来源文件进入跳过目录或经过路径跳转".into());
            }
            if !required.insert(file) {
                return Err("受保护来源文件重复登记".into());
            }
        }
        let source = Self {
            root: Directory::open(root, &mut required, &skipped)?,
        };
        if !required.is_empty() {
            return Err("来源目录缺少已登记的受保护文件".into());
        }
        source.checkpoint()?;
        Ok(source)
    }

    pub(in crate::recovery) fn checkpoint(&self) -> Result<()> {
        self.root.entry.check_identity()?;
        self.root.check()?;
        self.root.entry.check_identity()
    }
}

fn skip_names(skipped: &[&str]) -> Result<BTreeSet<OsString>> {
    let mut names = BTreeSet::new();
    for name in skipped {
        let name = OsStr::new(name);
        let mut components = Path::new(name).components();
        if components.next() != Some(Component::Normal(name))
            || components.next().is_some()
            || !names.insert(name.to_ascii_lowercase())
        {
            return Err("来源只能精确跳过根下唯一的目录名".into());
        }
    }
    Ok(names)
}

struct Directory {
    entry: FrozenEntry,
    members: BTreeMap<OsString, Member>,
}

enum Member {
    HeldFile(FrozenEntry),
    OtherFile(FileIdentity),
    Directory(Directory),
    SkippedDirectory(FrozenEntry),
}

#[derive(PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl From<Identity> for FileIdentity {
    fn from(identity: Identity) -> Self {
        Self {
            device: identity.device,
            inode: identity.inode,
        }
    }
}

impl Directory {
    fn open(
        entry: FrozenEntry,
        required: &mut BTreeSet<PathBuf>,
        skipped: &BTreeSet<OsString>,
    ) -> Result<Self> {
        entry.check_identity_with(path_handle_identity)?;
        let mut members = BTreeMap::new();
        for current in read_directory(&entry.path)? {
            let current = current?;
            let name = current.file_name();
            let path = entry.path.join(&name);
            let metadata = member_metadata(&current)?;
            let directory = metadata.is_dir();
            let member = if skipped.contains(&name.to_ascii_lowercase()) {
                if !directory {
                    return Err("跳过的来源目录入口必须为普通目录".into());
                }
                Member::SkippedDirectory(open_child(&path, true)?)
            } else if directory {
                Member::Directory(Self::open(
                    open_child(&path, true)?,
                    required,
                    &BTreeSet::new(),
                )?)
            } else if required.remove(&path) || is_git_control_file(&name) {
                Member::HeldFile(open_child(&path, false)?)
            } else {
                Member::OtherFile(path_handle_identity(&path, false)?.into())
            };
            if members.insert(name, member).is_some() {
                return Err("来源目录枚举包含重复成员".into());
            }
        }
        entry.check_identity_with(path_handle_identity)?;
        Ok(Self { entry, members })
    }

    fn check(&self) -> Result<()> {
        self.entry.check_identity_with(path_handle_identity)?;
        let mut names = BTreeSet::new();
        for current in read_directory(&self.entry.path)? {
            let current = current?;
            let name = current.file_name();
            let Some(member) = self.members.get(&name) else {
                return Err("受保护来源目录出现新成员".into());
            };
            if !names.insert(name) {
                return Err("受保护来源目录枚举包含重复成员".into());
            }
            member.check(&current)?;
        }
        if names.len() != self.members.len() {
            return Err("受保护来源目录缺少原成员".into());
        }
        self.entry.check_identity_with(path_handle_identity)
    }
}

fn is_git_control_file(name: &OsStr) -> bool {
    let name = name.to_ascii_lowercase();
    name == ".gitignore" || name == ".gitattributes"
}

fn read_directory(path: &Path) -> Result<fs::ReadDir> {
    Ok(fs::read_dir(common::windows_path(path)?)?)
}

impl Member {
    fn check(&self, current: &fs::DirEntry) -> Result<()> {
        let metadata = member_metadata(current)?;
        let directory = metadata.is_dir();
        match self {
            Self::HeldFile(file) if !directory => {
                // 父目录仍按路径核验，持有文件拒绝删除及修改。目录记录核对原名字、类型和大小。
                if metadata.len() != file.identity.bytes {
                    return Err("受保护来源目录枚举的文件大小发生变化".into());
                }
                file.check_held_identity()
            }
            Self::Directory(dir) if directory => dir.check(),
            Self::SkippedDirectory(dir) if directory => {
                dir.check_identity_with(path_handle_identity)
            }
            Self::OtherFile(expected) if !directory => {
                let actual = path_handle_identity(&current.path(), false)?;
                if FileIdentity::from(actual) != *expected {
                    return Err("受保护来源目录的普通成员身份发生变化".into());
                }
                Ok(())
            }
            _ => Err("受保护来源目录的成员类型发生变化".into()),
        }
    }
}

fn member_metadata(entry: &fs::DirEntry) -> Result<fs::Metadata> {
    let metadata = entry.metadata()?;
    if digest::linked(&metadata) || !(metadata.is_file() || metadata.is_dir()) {
        return Err("受保护来源目录成员必须为普通文件或目录".into());
    }
    Ok(metadata)
}

fn open_child(path: &Path, directory: bool) -> Result<FrozenEntry> {
    // 上级目录已经持有；这次仍通过真实路径及句柄核对节点身份。
    FrozenEntry::open_identity_with(path, directory, false, path_handle_identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let file = temporary.path().join("src").join("lib.rs");
        fs::create_dir(file.parent().unwrap()).unwrap();
        fs::write(&file, b"code").unwrap();
        (temporary, file)
    }

    #[test]
    fn held_source_denies_writers_and_keeps_unregistered_leaf_contents_unbound() {
        let (temporary, file) = fixture();
        let root = temporary.path();
        let git = root.join(".git");
        let marker = root.join("guard.json");
        let ignored = root.join("ignored.txt");
        let gitignore = root.join(".gitignore");
        let attributes = file.parent().unwrap().join(".gitattributes");
        for item in [&git, &marker, &ignored, &gitignore, &attributes] {
            fs::write(item, b"original").unwrap();
        }
        let writer = fs::OpenOptions::new().write(true).open(&file).unwrap();
        assert!(HeldSourceTree::open(root, [file.clone()], &[]).is_err());
        drop(writer);
        let held = HeldSourceTree::open(root, [file.clone(), git, marker.clone()], &[]).unwrap();
        assert!(fs::write(&file, b"same").is_err());
        let replacement = root.join("replacement.rs");
        fs::write(&replacement, b"same").unwrap();
        assert!(fs::rename(&replacement, &file).is_err());
        fs::remove_file(replacement).unwrap();
        assert!(fs::remove_file(&marker).is_err());
        assert!(fs::write(&gitignore, b"new ignore").is_err());
        assert!(fs::write(&attributes, b"new attributes").is_err());
        assert!(fs::rename(file.parent().unwrap(), root.join("new-src")).is_err());
        fs::write(&ignored, b"longer ignored contents").unwrap();
        held.checkpoint().unwrap();
        fs::rename(&ignored, root.join("renamed.txt")).unwrap();
        assert!(held.checkpoint().is_err());
        drop(held);
        fs::write(file, b"later").unwrap();
    }

    #[test]
    fn held_source_skips_only_bound_root_directory_contents() {
        let (temporary, file) = fixture();
        let root = temporary.path();
        let skipped = root.join("node_modules");
        fs::create_dir(&skipped).unwrap();
        let inside = skipped.join("package.js");
        fs::write(&inside, b"package").unwrap();
        assert!(HeldSourceTree::open(root, [inside], &["node_modules"]).is_err());
        assert!(HeldSourceTree::open(root, [root.join("missing")], &[]).is_err());
        fs::write(root.join("dist"), b"plain file").unwrap();
        assert!(HeldSourceTree::open(root, [file.clone()], &["dist"]).is_err());

        let held = HeldSourceTree::open(root, [file], &["node_modules", "not-present"]).unwrap();
        fs::write(skipped.join("new.js"), b"new package").unwrap();
        held.checkpoint().unwrap();
        assert!(fs::rename(&skipped, root.join("moved")).is_err());
        fs::create_dir(root.join("not-present")).unwrap();
        assert!(held.checkpoint().is_err());
    }

    #[test]
    fn held_source_rejects_new_nested_member_and_changed_node_type() {
        let (temporary, file) = fixture();
        let inactive = temporary.path().join("inactive");
        fs::write(&inactive, b"a").unwrap();
        let held = HeldSourceTree::open(temporary.path(), [file.clone()], &[]).unwrap();
        let extra = file.with_extension("extra");
        fs::write(&extra, b"extra").unwrap();
        assert!(held.checkpoint().is_err());
        fs::remove_file(extra).unwrap();
        held.checkpoint().unwrap();
        fs::remove_file(&inactive).unwrap();
        fs::create_dir(inactive).unwrap();
        assert!(held.checkpoint().is_err());
    }

    #[test]
    fn held_source_rejects_changed_hard_link_count_when_the_volume_allows_it() {
        let (temporary, file) = fixture();
        let skipped = temporary.path().join("dist");
        fs::create_dir(&skipped).unwrap();
        fs::hard_link(&file, skipped.join("prior.rs")).unwrap();
        let held = HeldSourceTree::open(temporary.path(), [file.clone()], &["dist"]).unwrap();
        held.checkpoint().unwrap();
        if fs::hard_link(&file, skipped.join("extra.rs")).is_ok() {
            assert!(held.checkpoint().is_err());
        } else {
            held.checkpoint().unwrap();
        }
    }
}
