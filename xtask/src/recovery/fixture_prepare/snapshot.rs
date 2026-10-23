use super::super::full_stack::Port;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::source,
};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

#[derive(PartialEq, Eq)]
pub(super) struct Snapshot {
    pub receipt: Value,
    pub patch: Vec<u8>,
    clean: bool,
}

pub(super) fn capture(root: &Path) -> Result<Snapshot> {
    let inventory = source::capture(root)?;
    let snapshot = inventory.source.snapshot;
    let patch = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--binary", "HEAD"])
        .output()?;
    if !patch.status.success()
        || crate::devex::runtime_native::artifacts::sha(&patch.stdout) != snapshot.patch_sha256
    {
        return Err("采集业务夹具补丁期间来源漂移".into());
    }
    let files = snapshot
        .files
        .iter()
        .map(|v| json!({"path":v.path,"sha256":v.sha256}))
        .collect::<Vec<_>>();
    Ok(Snapshot {
        receipt: json!({"head":snapshot.head,"patch_sha256":snapshot.patch_sha256,"files":files}),
        patch: patch.stdout,
        clean: snapshot.clean,
    })
}

pub(super) fn formal(snapshot: &Snapshot, expected: &str) -> Result<()> {
    if expected.len() != 40
        || !expected
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
        || !snapshot.clean
        || snapshot.receipt["head"] != expected
        || !snapshot.patch.is_empty()
        || snapshot.receipt["files"] != json!([])
    {
        return Err("正式夹具必须使用精确、干净的候选提交".into());
    }
    Ok(())
}

pub(super) fn copy(
    source: &Path,
    destination: &Path,
    snapshot: &Snapshot,
    port: &mut impl Port,
) -> Result<()> {
    let head = snapshot.receipt["head"].as_str().ok_or("源码缺少 HEAD")?;
    port.command(
        "git",
        &[
            "-C".into(),
            source.to_string_lossy().into_owned(),
            "worktree".into(),
            "add".into(),
            "--detach".into(),
            destination.to_string_lossy().into_owned(),
            head.into(),
        ],
        None,
    )?;
    if !snapshot.patch.is_empty() {
        // Git apply 的二进制补丁通过受管文件传递，不进入 argv 或日志。
        let mut patch = tempfile::NamedTempFile::new_in(destination.parent().ok_or("缺少输出根")?)?;
        use std::io::Write;
        patch.write_all(&snapshot.patch)?;
        patch.as_file().sync_all()?;
        for check in [true, false] {
            let mut args = vec![
                "-C".into(),
                destination.to_string_lossy().into_owned(),
                "apply".into(),
            ];
            if check {
                args.push("--check".into());
            }
            args.extend([
                "--binary".into(),
                patch.path().to_string_lossy().into_owned(),
            ]);
            port.command("git", &args, None)?;
        }
    }
    for entry in snapshot.receipt["files"].as_array().ok_or("源码清单缺失")? {
        let name = entry["path"].as_str().ok_or("源码路径无效")?;
        let input = source.join(name);
        let output = destination.join(name);
        validate_absolute_path(&input, LocalTestPathKind::ExistingFile)?;
        if !input.starts_with(source)
            || !output.starts_with(destination)
            || name.split('/').any(|v| v == "..")
        {
            return Err("未跟踪文件路径越界".into());
        }
        validate_absolute_path(&output, LocalTestPathKind::OutputFile)?;
        fs::create_dir_all(output.parent().ok_or("未跟踪路径缺少父目录")?)?;
        fs::copy(input, output)?;
    }
    if capture(source)?.receipt != snapshot.receipt
        || capture(destination)?.receipt != snapshot.receipt
    {
        return Err("工作树复制期间来源漂移；保留夹具，不使用混合快照".into());
    }
    Ok(())
}
