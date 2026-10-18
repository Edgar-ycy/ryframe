use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use serde_json::Value;

use crate::{
    Result,
    recovery::clone::native::{digest, resources::common},
};

pub(crate) fn terminal(directory: &Path, value: &Value) -> Result<()> {
    terminal_with_cleanup(directory, value, |path| Ok(fs::remove_file(path)?))
}

/// pending 删除成功才是最终提交点；任一步骤失败均保留现场供显式核对。
pub(crate) fn terminal_with_cleanup(
    directory: &Path,
    value: &Value,
    remove_pending: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    crate::local_test_path::validate_absolute_path(
        directory,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".result.json.") && name.ends_with(".pending") {
            return Err("监控最终成功收据存在待核对 pending，拒绝重放".into());
        }
    }
    let target = directory.join("result.json");
    require_absent(&target)?;
    let mut content = serde_json::to_vec_pretty(value)?;
    content.push(b'\n');
    if content.len() > 1024 * 1024 {
        return Err("监控最终成功收据超过 1 MiB".into());
    }
    let pending = directory.join(format!(
        ".result.json.{}.pending",
        uuid::Uuid::new_v4().simple()
    ));
    let mut stream = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)?;
    stream.write_all(&content)?;
    stream.sync_all()?;
    drop(stream);
    let before = state(&pending, content.len(), 1)?;
    if fs::read(&pending)? != content || state(&pending, content.len(), 1)? != before {
        return Err("监控最终成功收据临时文件在提交前变化".into());
    }
    require_absent(&target)?;
    fs::hard_link(&pending, &target)
        .map_err(|error| format!("监控最终成功收据被并发创建或无法发布：{error}"))?;
    let linked = state(&pending, content.len(), 2)?;
    if linked != before
        || state(&target, content.len(), 2)? != before
        || fs::read(&target)? != content
        || state(&pending, content.len(), 2)? != before
        || state(&target, content.len(), 2)? != before
    {
        return Err("监控最终成功收据发布后文件身份或内容无效，保留 pending 供核对".into());
    }
    remove_pending(&pending)
}

fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err("监控最终成功收据已存在，拒绝覆盖或重放".into()),
        Err(error) => Err(error.into()),
    }
}

fn state(path: &Path, bytes: usize, links: u64) -> Result<(u64, u64, std::time::SystemTime)> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || digest::linked(&metadata)
        || metadata.len() != bytes as u64
        || common::link_count_for(path, &metadata)? != links
    {
        return Err("监控最终成功收据文件类型、大小或硬链接数量无效".into());
    }
    let (device, inode) = common::device_inode_for(path, &metadata)?;
    Ok((device, inode, metadata.modified()?))
}
