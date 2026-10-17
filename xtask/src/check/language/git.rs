use std::{
    collections::BTreeSet,
    ffi::OsStr,
    path::{Component, Path},
    process::Stdio,
};

use crate::{Result, process::child_command};

pub(crate) const FILES: &[&str] = &[
    "ls-files",
    "--cached",
    "--others",
    "--exclude-standard",
    "--deduplicate",
    "-z",
];
pub(crate) const IGNORED: &[&str] = &[
    "ls-files",
    "--cached",
    "--ignored",
    "--exclude-standard",
    "-z",
];
pub(crate) const DELETED: &[&str] = &["ls-files", "--deleted", "-z"];
pub(crate) const HEAD: &[&str] = &["rev-parse", "--verify", "--quiet", "HEAD"];

#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) code: i32,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

pub(crate) fn invoke(root: &Path, args: &[&str]) -> Result<Reply> {
    let mut command = child_command(OsStr::new("git"));
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(variable);
    }
    let output = command
        .args([
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
        ])
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("语言报告无法执行 Git {}：{error}", args.join(" ")))?;
    Ok(Reply {
        code: output.status.code().unwrap_or(-1),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

pub(crate) fn success(reply: Reply) -> Result<Vec<u8>> {
    if reply.code != 0 {
        return Err(format!(
            "语言报告 Git 失败（{}）：{}",
            reply.code,
            String::from_utf8_lossy(&reply.stderr)
        )
        .into());
    }
    Ok(reply.stdout)
}

pub(crate) fn paths(bytes: &[u8]) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    if bytes.is_empty() {
        return Ok(paths);
    }
    let body = bytes
        .strip_suffix(&[0])
        .ok_or("语言报告 Git 路径列表未以 NUL 终结")?;
    for bytes in body.split(|byte| *byte == 0) {
        if bytes.is_empty() {
            return Err("语言报告 Git 路径列表含空路径".into());
        }
        let value = std::str::from_utf8(bytes)?;
        let path = Path::new(value);
        if path.is_absolute()
            || value.contains('\\')
            || value.contains(':')
            || value.split('/').any(|part| matches!(part, "" | "." | ".."))
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(format!("Git 返回非规范仓库相对路径：{value:?}").into());
        }
        paths.insert(value.into());
    }
    Ok(paths)
}
