//! Windows 恢复期间保护原 Git 观察依赖的工作树元数据。
use std::{
    collections::BTreeSet,
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::target_verify::guard::BackupInputs,
};

use super::git;

const UNSUPPORTED: &str = "Git 元数据布局或配置不支持持续保护，需要原完整来源核验";
const GIT_ENVIRONMENT: &[&str] = &[
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "XDG_CONFIG_HOME",
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "PROGRAMDATA",
];

pub(super) struct HeldGit {
    head: String,
    protected: BackupInputs,
    absent: BTreeSet<PathBuf>,
    environment: Vec<Option<std::ffi::OsString>>,
}

impl HeldGit {
    pub(super) fn open(root: &Path) -> Result<Self> {
        let environment = git_environment();
        let root = canonical(root, LocalTestPathKind::ExistingDirectory)?;
        let before = Layout::read(&root)?;
        let external = external_files(&root)?;
        let metadata = collect(&before, &external, &root)?;
        let held = Self {
            head: before.head.clone(),
            protected: BackupInputs::open(
                &root,
                metadata.directories.iter().cloned(),
                metadata.files.iter().cloned(),
            )?,
            absent: metadata.absent.clone(),
            environment,
        };
        held.checkpoint()?;
        if Layout::read(&root)? != before
            || external_files(&root)? != external
            || collect(&before, &external, &root)? != metadata
        {
            return Err("Git 路径、HEAD 或配置来源在取得保护期间发生变化".into());
        }
        verify_configuration(&root)?;
        verify_index_and_attributes(&root)?;
        held.checkpoint()?;
        Ok(held)
    }

    pub(super) fn head(&self) -> &str {
        &self.head
    }

    pub(super) fn checkpoint(&self) -> Result<()> {
        self.protected.checkpoint()?;
        for path in &self.absent {
            if !missing(path)? {
                return Err("持续保护期间出现原本缺省的 Git 元数据".into());
            }
        }
        if git_environment() != self.environment {
            return Err("持续保护期间 Git 本机环境发生变化".into());
        }
        Ok(())
    }
}

#[derive(PartialEq, Eq)]
struct Layout {
    git_dir: PathBuf,
    common_dir: PathBuf,
    head: String,
}

impl Layout {
    fn read(root: &Path) -> Result<Self> {
        let args = [
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
            "--git-path",
            "index",
            "HEAD",
        ];
        let isolated = git::run(root, &args)?;
        let original = git::observation(root, &args)?;
        if isolated != original {
            return Err(UNSUPPORTED.into());
        }
        let value = std::str::from_utf8(&original)?;
        let lines = value.lines().collect::<Vec<_>>();
        let [work, dir, common, index, head] = lines.as_slice() else {
            return Err(UNSUPPORTED.into());
        };
        if canonical(Path::new(work), LocalTestPathKind::ExistingDirectory)? != root {
            return Err(UNSUPPORTED.into());
        }
        let git_dir = canonical(Path::new(dir), LocalTestPathKind::ExistingDirectory)?;
        let common_dir = canonical(Path::new(common), LocalTestPathKind::ExistingDirectory)?;
        if canonical(Path::new(index), LocalTestPathKind::ExistingFile)? != git_dir.join("index")
            || detached_head(&git_dir.join("HEAD"))? != *head
        {
            return Err(UNSUPPORTED.into());
        }
        Ok(Self {
            git_dir,
            common_dir,
            head: (*head).to_owned(),
        })
    }

    fn collect(&self, root: &Path, metadata: &mut Metadata) -> Result<()> {
        let dot = root.join(".git");
        metadata.directories.insert(self.git_dir.clone());
        metadata.directories.insert(self.common_dir.clone());
        if self.git_dir == self.common_dir {
            if canonical(&dot, LocalTestPathKind::ExistingDirectory)? != self.git_dir {
                return Err(UNSUPPORTED.into());
            }
            metadata.reject(self.git_dir.join("commondir"))?;
            metadata.reject(self.git_dir.join("gitdir"))?;
        } else {
            self.collect_worktree(&dot, metadata)?;
        }
        for name in ["HEAD", "index"] {
            metadata.file(&self.git_dir.join(name))?;
        }
        metadata.file(&self.common_dir.join("config"))?;
        metadata.optional(self.common_dir.join("shallow"))?;
        for directory in [&self.git_dir, &self.common_dir] {
            for name in ["info/exclude", "info/attributes"] {
                metadata.optional(directory.join(name))?;
            }
            for name in [
                "info/sparse-checkout",
                "config.worktree",
                "info/grafts",
                "rebase-apply",
                "rebase-merge",
                "MERGE_HEAD",
                "CHERRY_PICK_HEAD",
                "REVERT_HEAD",
                "sequencer",
            ] {
                metadata.reject(directory.join(name))?;
            }
        }
        for name in ["objects/info/alternates", "objects/info/http-alternates"] {
            metadata.reject(self.common_dir.join(name))?;
        }
        Ok(())
    }

    fn collect_worktree(&self, dot: &Path, metadata: &mut Metadata) -> Result<()> {
        if self.git_dir.parent() != Some(self.common_dir.join("worktrees").as_path()) {
            return Err(UNSUPPORTED.into());
        }
        metadata.file(dot)?;
        let link = read_line(dot)?;
        let target = Path::new(link.strip_prefix("gitdir: ").ok_or(UNSUPPORTED)?);
        let reverse = self.git_dir.join("gitdir");
        let common = self.git_dir.join("commondir");
        metadata.file(&reverse)?;
        metadata.file(&common)?;
        let origin = read_line(&reverse)?;
        if canonical(target, LocalTestPathKind::ExistingDirectory)? != self.git_dir
            || canonical(Path::new(&origin), LocalTestPathKind::ExistingFile)?
                != canonical(dot, LocalTestPathKind::ExistingFile)?
            || !matches!(read_line(&common)?.as_str(), "../.." | "..\\..")
        {
            return Err(UNSUPPORTED.into());
        }
        Ok(())
    }
}

fn collect(layout: &Layout, external: &ExternalFiles, root: &Path) -> Result<Metadata> {
    let mut metadata = Metadata::default();
    layout.collect(root, &mut metadata)?;
    external.collect(&mut metadata)?;
    Ok(metadata)
}

#[derive(Default, PartialEq, Eq)]
struct Metadata {
    directories: BTreeSet<PathBuf>,
    files: BTreeSet<PathBuf>,
    absent: BTreeSet<PathBuf>,
}

impl Metadata {
    fn file(&mut self, path: &Path) -> Result<()> {
        validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
        self.add_anchor(path)?;
        self.files.insert(path.to_owned());
        Ok(())
    }

    fn optional(&mut self, path: PathBuf) -> Result<()> {
        if missing(&path)? {
            self.record_missing(path)?;
        } else {
            self.file(&path)?;
        }
        Ok(())
    }

    fn reject(&mut self, path: PathBuf) -> Result<()> {
        if !missing(&path)? {
            return Err(UNSUPPORTED.into());
        }
        self.record_missing(path)
    }

    fn record_missing(&mut self, path: PathBuf) -> Result<()> {
        validate_absolute_path(&path, LocalTestPathKind::OutputFile)?;
        for parent in path.ancestors().skip(1) {
            if !missing(parent)? {
                self.directories
                    .insert(canonical(parent, LocalTestPathKind::ExistingDirectory)?);
                self.absent.insert(path);
                return Ok(());
            }
            self.absent.insert(parent.to_owned());
        }
        Err(UNSUPPORTED.into())
    }

    fn add_anchor(&mut self, path: &Path) -> Result<()> {
        for parent in path.ancestors().skip(1) {
            if !missing(parent)? {
                self.directories
                    .insert(canonical(parent, LocalTestPathKind::ExistingDirectory)?);
                return Ok(());
            }
        }
        Err(UNSUPPORTED.into())
    }
}

#[derive(PartialEq, Eq)]
struct ExternalFiles {
    configuration: Vec<PathBuf>,
    attributes: Vec<PathBuf>,
    exclude: PathBuf,
}

impl ExternalFiles {
    fn collect(&self, metadata: &mut Metadata) -> Result<()> {
        for path in self
            .configuration
            .iter()
            .chain(&self.attributes)
            .chain(std::iter::once(&self.exclude))
        {
            metadata.optional(path.clone())?;
        }
        Ok(())
    }
}

fn external_files(root: &Path) -> Result<ExternalFiles> {
    let paths = |key| -> Result<Vec<PathBuf>> {
        let output = git::observation(root, &["var", key])?;
        let paths = std::str::from_utf8(&output)?
            .lines()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if paths.is_empty() || paths.iter().any(|path| !path.is_absolute()) {
            return Err(UNSUPPORTED.into());
        }
        Ok(paths)
    };
    let mut configuration = paths("GIT_CONFIG_SYSTEM")?;
    configuration.extend(paths("GIT_CONFIG_GLOBAL")?);
    let mut attributes = paths("GIT_ATTR_SYSTEM")?;
    let global = paths("GIT_ATTR_GLOBAL")?;
    let [user] = global.as_slice() else {
        return Err(UNSUPPORTED.into());
    };
    if user.file_name().and_then(|name| name.to_str()) != Some("attributes") {
        return Err(UNSUPPORTED.into());
    }
    let exclude = user.with_file_name("ignore");
    attributes.extend(global);
    Ok(ExternalFiles {
        configuration,
        attributes,
        exclude,
    })
}

fn verify_configuration(root: &Path) -> Result<()> {
    let names = git::observation(
        root,
        &["config", "--null", "--name-only", "--list", "--no-includes"],
    )?;
    for name in std::str::from_utf8(&names)?.split_terminator('\0') {
        let name = name.to_ascii_lowercase();
        let (section, _) = name.split_once('.').ok_or(UNSUPPORTED)?;
        let accepted = match section {
            "core" => matches!(
                name.as_str(),
                "core.repositoryformatversion"
                    | "core.bare"
                    | "core.filemode"
                    | "core.symlinks"
                    | "core.ignorecase"
                    | "core.logallrefupdates"
                    | "core.longpaths"
                    | "core.autocrlf"
                    | "core.eol"
                    | "core.safecrlf"
                    | "core.fscache"
                    | "core.hookspath"
            ),
            "diff" => name == "diff.astextplain.textconv",
            "filter" => name.starts_with("filter.lfs."),
            "branch" | "remote" | "user" | "init" | "pull" | "http" | "https" | "credential"
            | "safe" | "alias" => true,
            _ => false,
        };
        if !accepted {
            return Err(UNSUPPORTED.into());
        }
    }
    Ok(())
}

fn verify_index_and_attributes(root: &Path) -> Result<()> {
    if !git::observation(root, &["rev-parse", "--shared-index-path"])?.is_empty() {
        return Err(UNSUPPORTED.into());
    }
    let index = git::observation(root, &["ls-files", "--stage", "--sparse", "-z"])?;
    for entry in index
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let prefix = entry
            .split(|byte| *byte == b'\t')
            .next()
            .ok_or(UNSUPPORTED)?;
        if !(prefix.starts_with(b"100644 ") || prefix.starts_with(b"100755 "))
            || !prefix.ends_with(b" 0")
        {
            return Err(UNSUPPORTED.into());
        }
    }
    let paths = git::observation(
        root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let paths = std::str::from_utf8(&paths)?
        .split_terminator('\0')
        .collect::<BTreeSet<_>>();
    const ATTRIBUTES: [&str; 5] = ["filter", "diff", "working-tree-encoding", "ident", "eol"];
    let mut arguments = vec!["check-attr", "-z"];
    arguments.extend(ATTRIBUTES);
    arguments.push("--");
    let first_path = arguments.len();
    let mut length = 0;
    for path in paths {
        // 短批避免超过 Windows 进程命令行上限，路径只能作为选项结束符后的参数。
        if length + path.len() > 12_000 && length != 0 {
            verify_attributes(root, &arguments, &ATTRIBUTES, first_path)?;
            arguments.truncate(first_path);
            length = 0;
        }
        arguments.push(path);
        length += path.len() + 2;
    }
    if length != 0 {
        verify_attributes(root, &arguments, &ATTRIBUTES, first_path)?;
    }
    Ok(())
}

fn verify_attributes(
    root: &Path,
    arguments: &[&str],
    attributes: &[&str],
    first_path: usize,
) -> Result<()> {
    let output = git::observation(root, arguments)?;
    let rows = output.strip_suffix(b"\0").ok_or(UNSUPPORTED)?;
    let mut fields = rows.split(|byte| *byte == 0);
    for path in &arguments[first_path..] {
        for attribute in attributes {
            if fields.next() != Some(path.as_bytes()) || fields.next() != Some(attribute.as_bytes())
            {
                return Err(UNSUPPORTED.into());
            }
            match fields.next() {
                Some(b"unspecified" | b"unset") => {}
                // 固定属性文件声明的内建文本转换不启动外部程序。
                Some(value)
                    if !value.is_empty()
                        && matches!(*attribute, "working-tree-encoding" | "ident" | "eol") => {}
                Some(b"set") if *attribute == "diff" => {}
                _ => return Err(UNSUPPORTED.into()),
            }
        }
    }
    if fields.next().is_some() {
        return Err(UNSUPPORTED.into());
    }
    Ok(())
}

fn git_environment() -> Vec<Option<std::ffi::OsString>> {
    GIT_ENVIRONMENT.iter().map(env::var_os).collect()
}

fn canonical(path: &Path, kind: LocalTestPathKind) -> Result<PathBuf> {
    validate_absolute_path(path, kind)?;
    Ok(super::super::super::portable_path(&fs::canonicalize(path)?))
}

fn missing(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn detached_head(path: &Path) -> Result<String> {
    let head = read_line(path)?;
    if head.len() != 40
        || !head
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(UNSUPPORTED.into());
    }
    Ok(head)
}

fn read_line(path: &Path) -> Result<String> {
    validate_absolute_path(path, LocalTestPathKind::ExistingFile)?;
    let text = fs::read_to_string(path)?;
    let line = text.strip_suffix('\n').ok_or(UNSUPPORTED)?;
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.is_empty() || line.contains(['\r', '\n', '\0']) {
        return Err(UNSUPPORTED.into());
    }
    Ok(line.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary() -> tempfile::TempDir {
        let local = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        tempfile::tempdir_in(local).unwrap()
    }

    fn fixture(temporary: &tempfile::TempDir) -> PathBuf {
        let root = temporary.path().join("repo");
        fs::create_dir(&root).unwrap();
        git::run(&root, &["init", "--quiet", "--template="]).unwrap();
        fs::write(root.join("data.txt"), b"original\n").unwrap();
        git::run(&root, &["add", "--", "data.txt"]).unwrap();
        git::run(
            &root,
            &[
                "-c",
                "user.name=RyFrame",
                "-c",
                "user.email=test@ryframe.invalid",
                "commit",
                "--quiet",
                "--no-gpg-sign",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
        git::run(&root, &["checkout", "--quiet", "--detach", "HEAD"]).unwrap();
        root
    }

    #[test]
    fn held_git_protects_detached_head_index_and_optional_attributes() {
        let temporary = temporary();
        let root = fixture(&temporary);
        let dot = root.join(".git");
        let info = dot.join("info");
        fs::create_dir(&info).unwrap();
        let attributes = info.join("attributes");
        let exclude = info.join("exclude");
        fs::write(&exclude, b"scratch\n").unwrap();

        let held = HeldGit::open(&root).unwrap();
        assert_eq!(
            held.head(),
            git::text(&root, &["rev-parse", "HEAD"]).unwrap()
        );
        assert!(fs::write(dot.join("HEAD"), b"ref: refs/heads/other\n").is_err());
        assert!(fs::rename(dot.join("index"), dot.join("index.other")).is_err());
        assert!(fs::write(&exclude, b"new\n").is_err());
        held.checkpoint().unwrap();
        fs::write(attributes, b"data.txt -diff\n").unwrap();
        assert!(held.checkpoint().is_err());
        drop(held);

        git::run(&root, &["checkout", "--quiet", "-b", "symbolic"]).unwrap();
        assert!(HeldGit::open(&root).is_err());
    }

    #[test]
    fn held_linked_git_protects_gitfile_and_common_configuration() {
        let temporary = temporary();
        let root = fixture(&temporary);
        let linked = temporary.path().join("linked");
        git::run(
            &root,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        )
        .unwrap();
        let held = HeldGit::open(&linked).unwrap();
        assert!(fs::write(linked.join(".git"), b"gitdir: another\n").is_err());
        assert!(fs::write(root.join(".git/config"), b"[core]\n").is_err());
        held.checkpoint().unwrap();
        drop(held);
        git::run(&root, &["config", "--local", "include.path", "other.conf"]).unwrap();
        assert!(HeldGit::open(&linked).is_err());
    }
}
