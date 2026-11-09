use std::{fs, io::Write, path::Path, process::Command};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::source,
};

use super::{assets, input::Side};

pub(super) fn run(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = command(root);
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args([
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.hooksPath=.git/disabled-hooks",
        ]);
    output(command, args)
}

pub(super) fn patch(root: &Path) -> Result<Vec<u8>> {
    // 原补丁必须沿用 source::capture 的 Git 观察语义，不能换全局配置后得到另一份摘要。
    output(
        command(root),
        &["diff", "--binary", "--no-ext-diff", "HEAD", "--", "."],
    )
}

pub(super) fn require_ignored(root: &Path, directory: &str) -> Result<()> {
    // 与完整来源观察使用同一 Git 配置，不能仅凭受管目录名跳过源码检查。
    output(command(root), &["check-ignore", "--quiet", "--", directory])
        .map(|_| ())
        .map_err(|error| {
            format!("受管资产目录未通过 Git ignore 核验；检查忽略规则及 Git 可用性：{error}").into()
        })
}

fn command(root: &Path) -> Command {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    command
        .arg("-C")
        .arg(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    command
}

fn output(mut command: Command, args: &[&str]) -> Result<Vec<u8>> {
    let result = command.args(args).output().map_err(|error| {
        format!(
            "执行源码 Git 阶段启动失败（{:?}，OS {:?}）；检查 Git 安装和目录权限",
            error.kind(),
            error.raw_os_error()
        )
    })?;
    if !result.status.success() {
        return Err(format!(
            "执行源码 Git {} 阶段失败（{}）：{}；保留输出，禁止覆盖或自动重放",
            args.first().unwrap_or(&"unknown"),
            result.status,
            failure_reason(&result.stderr)
        )
        .into());
    }
    Ok(result.stdout)
}

fn failure_reason(stderr: &[u8]) -> &'static str {
    // 只报告已识别的错误类别，不回显可能含凭据或用户数据的 Git 原文。
    let text = String::from_utf8_lossy(&stderr[..stderr.len().min(16 * 1024)]).to_ascii_lowercase();
    if text.contains("filename too long") || text.contains("file name too long") {
        "文件路径超过 Git 长路径配置；核对新仓库 core.longpaths 后选择新输出目录"
    } else if text.contains("out of memory") || text.contains("cannot allocate memory") {
        "Git 可用内存不足；释放内存后使用新输出目录"
    } else if text.contains("dubious ownership") {
        "Git 拒绝当前目录归属；核对目录所有者和执行身份，不绕过来源校验"
    } else if text.contains("permission denied")
        || text.contains("access is denied")
        || text.contains("couldn't create signal pipe")
    {
        "Git 子进程或文件访问被执行环境拒绝；核对本机权限与沙箱限制后使用新输出目录"
    } else {
        "Git 未能完成本地源码操作；核对当前提交对象、路径、权限和可用内存，原因尚待定位"
    }
}

pub(super) fn text(root: &Path, args: &[&str]) -> Result<String> {
    let value = String::from_utf8(run(root, args)?).map_err(|_| "Git 输出不是 UTF-8")?;
    Ok(value.trim_end().to_owned())
}

pub(super) fn publish(side: &Side, destination: &Path) -> Result<()> {
    let root = &side.root;
    let inventory = &side.inventory;
    let patch = &side.patch;
    require_regular_tree(root, &inventory.source.snapshot.head)?;
    fs::create_dir(destination)?;
    run(destination, &["init", "--quiet", "--template="])?;
    #[cfg(windows)]
    run(
        destination,
        &["config", "--local", "core.longpaths", "true"],
    )?;
    let source_path = root.to_str().ok_or("原源码路径必须为 UTF-8")?;
    let head = &inventory.source.snapshot.head;
    run(
        destination,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--depth=1",
            "--",
            source_path,
            head,
        ],
    )?;
    run(destination, &["checkout", "--quiet", "--detach", head])?;
    if !patch.is_empty() {
        let mut file =
            tempfile::NamedTempFile::new_in(destination.parent().ok_or("发布根缺少父目录")?)?;
        file.write_all(patch)?;
        file.as_file().sync_all()?;
        let path = file.path().to_str().ok_or("补丁路径必须为 UTF-8")?;
        run(destination, &["apply", "--check", "--binary", path])?;
        run(destination, &["apply", "--binary", path])?;
    }
    // 所有文件均以冻结清单为准；覆盖 checkout 的平台换行结果，不复制 ignored 内容。
    for entry in &inventory.files {
        let input = root.join(&entry.path);
        let output = destination.join(&entry.path);
        validate_absolute_path(&input, LocalTestPathKind::ExistingFile)?;
        let kind = if output.exists() {
            LocalTestPathKind::ExistingFile
        } else {
            LocalTestPathKind::OutputFile
        };
        validate_absolute_path(&output, kind)?;
        fs::create_dir_all(output.parent().ok_or("源码文件缺少父目录")?)?;
        fs::copy(input, output)?;
    }
    assets::copy(root, destination, &side.asset)?;
    if source::capture(root)? != *inventory
        || source::capture(destination)?.files != inventory.files
    {
        return Err("发布复制期间原源码变化或独立根内容不一致".into());
    }
    run(destination, &["add", "--all", "--", "."])?;
    if !side.asset.is_null() {
        run(destination, &["add", "--force", "--", assets::MARKER])?;
    }
    commit(destination)?;
    let actual = source::capture(destination)?;
    if !actual.source.snapshot.clean || actual.files != side.files {
        return Err("发布后的独立源码不干净或契约资产清单不一致".into());
    }
    Ok(())
}

fn commit(destination: &Path) -> Result<()> {
    run(
        destination,
        &[
            "-c",
            "user.name=RyFrame",
            "-c",
            "user.email=source@ryframe.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "--no-gpg-sign",
            "-m",
            "build(source): 发布独立业务执行源码",
        ],
    )?;
    Ok(())
}

fn require_regular_tree(root: &Path, head: &str) -> Result<()> {
    let tree = run(root, &["ls-tree", "-r", "-z", head])?;
    for entry in tree.split(|v| *v == 0).filter(|v| !v.is_empty()) {
        if !entry.starts_with(b"100644 blob ") && !entry.starts_with(b"100755 blob ") {
            return Err("原始 HEAD 含链接、子模块或非普通源码；禁止发布混合来源".into());
        }
    }
    Ok(())
}
