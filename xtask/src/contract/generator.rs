use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    process::{child_command, run},
    recovery::clone::native::resources::common::link_count_for,
};

const PATHS: [&str; 2] = [
    "scripts/generate-api-artifacts.mjs",
    "scripts/generate-api-artifacts.ts",
];

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct GeneratorEntry {
    worktree: PathBuf,
    git_directory: PathBuf,
    index_record: String,
    relative: &'static str,
    bytes: Vec<u8>,
}

impl GeneratorEntry {
    pub(crate) fn capture(frontend: &Path) -> Result<Self> {
        // 显式配置允许使用相邻仓库的相对路径。选定输入后只从真实 Git 根读取。
        let worktree = frontend.canonicalize()?;
        let git_directory = checkout_git_directory(&worktree)?;
        let index = git(
            &worktree,
            &["ls-files", "--stage", "-z", "--", PATHS[0], PATHS[1]],
        )?;
        let (relative, index_record) = index_entry(&index)?;
        let (actual, bytes) = read_entry(&worktree)?;
        if relative != actual {
            return Err("前端契约生成入口与 Git 工作区索引不一致".into());
        }
        Ok(Self {
            worktree,
            git_directory,
            index_record: index_record.to_owned(),
            relative,
            bytes,
        })
    }

    pub(crate) fn arguments(&self) -> [&str; 2] {
        [self.relative, "--write"]
    }

    pub(crate) fn verify_source(&self, frontend: &Path) -> Result<()> {
        if Self::capture(frontend)? != *self {
            return Err("前端契约生成入口或 Git 来源在事务期间发生变化".into());
        }
        Ok(())
    }

    pub(crate) fn verify_staging(&self, staging: &Path) -> Result<()> {
        let name = staging.file_name().ok_or("前端契约暂存目录缺少名称")?;
        let expected = self.worktree.join(".local-tests").join(name);
        validate_absolute_path(&expected, LocalTestPathKind::ExistingDirectory)?;
        if staging.canonicalize()? != expected.canonicalize()? {
            return Err("前端契约暂存目录不属于已核验工作区".into());
        }
        let (relative, bytes) = read_entry(&expected)?;
        if relative != self.relative || bytes != self.bytes {
            return Err("前端契约生成入口与原暂存输入的路径或字节不一致".into());
        }
        Ok(())
    }

    pub(super) fn run(&self, frontend: &Path, staging: &Path) -> Result<()> {
        self.verify(frontend, staging)?;
        let executed = run(staging, "node", &self.arguments());
        let after = self.verify(frontend, staging);
        executed.and(after)
    }

    pub(super) fn verify(&self, frontend: &Path, staging: &Path) -> Result<()> {
        self.verify_source(frontend)?;
        self.verify_staging(staging)
    }
}

fn checkout_git_directory(worktree: &Path) -> Result<PathBuf> {
    let identity = git(
        worktree,
        &["rev-parse", "--show-toplevel", "--absolute-git-dir"],
    )?;
    let mut paths = identity.lines();
    let root = paths.next().ok_or("Git 缺少前端工作区根目录")?;
    let directory = paths.next().ok_or("Git 缺少前端元数据目录")?;
    let (root, directory) = (Path::new(root), Path::new(directory));
    if paths.next().is_some()
        || !root.is_absolute()
        || !directory.is_absolute()
        || root.canonicalize()? != worktree
    {
        return Err("前端契约生成必须绑定实际的 Git 工作区根目录".into());
    }
    Ok(directory.canonicalize()?)
}

fn index_entry(index: &str) -> Result<(&'static str, &str)> {
    let record = index
        .strip_suffix('\0')
        .filter(|record| !record.contains('\0'))
        .ok_or("Git 索引必须有且只有一个已登记的前端契约生成入口")?;
    let (metadata, relative) = record.split_once('\t').ok_or("前端契约生成入口索引无效")?;
    let mut fields = metadata.split(' ');
    let (Some("100644" | "100755"), Some(object), Some("0"), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err("前端契约生成入口索引必须是无冲突的普通文件".into());
    };
    if !matches!(object.len(), 40 | 64) || !object.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("前端契约生成入口索引对象无效".into());
    }
    let relative = PATHS
        .into_iter()
        .find(|expected| *expected == relative)
        .ok_or("Git 索引包含未知的前端契约生成入口")?;
    Ok((relative, record))
}

fn read_entry(root: &Path) -> Result<(&'static str, Vec<u8>)> {
    validate_absolute_path(&root.join("scripts"), LocalTestPathKind::ExistingDirectory)?;
    let mut found = None;
    for relative in PATHS {
        let path = root.join(relative);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if found.replace((relative, path, metadata)).is_some() {
                    return Err("工作区同时存在两个前端契约生成入口".into());
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let (relative, path, metadata) = found.ok_or("工作区缺少前端契约生成入口")?;
    validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
    if link_count_for(&path, &metadata)? != 1 {
        return Err("前端契约生成入口不能使用硬链接".into());
    }
    Ok((relative, fs::read(path)?))
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut command = child_command("git");
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
        .args([
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(args)
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err("无法核验前端契约生成入口的实际 Git 工作区或索引".into());
    }
    Ok(String::from_utf8(output.stdout)?)
}
