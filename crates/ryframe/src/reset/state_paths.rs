//! 状态目录入口始终显式传入，检查全部已有路径层级及最终状态文件。

use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path},
};

use super::{ResetError, ResetResult};

pub(super) fn directory(path: &Path) -> ResetResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(ResetError::new(
            "reset.state_directory 必须是无点号跳转的绝对路径；请修正所选配置文件",
        ));
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() && !linked(&metadata) => {}
            Ok(_) => {
                return Err(ResetError::new(
                    "reset.state_directory 各层必须为普通目录，不能包含链接或重解析点",
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => {
                return Err(ResetError::new(
                    "无法核验 reset.state_directory；请检查目录类型与读取权限",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn file(path: &Path) -> ResetResult<()> {
    directory(
        path.parent()
            .ok_or_else(|| ResetError::new("reset 状态文件缺少父目录"))?,
    )?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !linked(&metadata) => Ok(()),
        Ok(_) => Err(ResetError::new(
            "reset 状态文件不能是目录、链接或重解析点；原证据保持不变",
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ResetError::new("无法核验 reset 状态文件身份或读取权限")),
    }
}

fn linked(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
