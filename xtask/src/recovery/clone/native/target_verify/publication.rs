use std::{
    fs::File,
    io::{self, Write},
    path::Path,
};

use serde_json::Value;

use super::super::json as json_io;
use crate::Result;

const LABEL: &str = "目标复核收据";

/// 完整序列化并同步临时收据后才发布，不覆盖已有最终路径。
pub(crate) fn publish(path: &Path, value: &Value) -> Result<()> {
    publish_with(
        path,
        value,
        |file, bytes| file.write_all(bytes),
        File::sync_all,
    )
}

pub(crate) fn publish_from(path: &Path, value: &Value, staging: &Path) -> Result<()> {
    json_io::publish_new_with(
        path,
        value,
        LABEL,
        Some(staging),
        |file, bytes| file.write_all(bytes),
        File::sync_all,
    )
}

/// 写入与同步钩子用于确定性核验发布前的失败和并发创建边界。
pub(crate) fn publish_with(
    path: &Path,
    value: &Value,
    writer: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    sync: impl FnOnce(&File) -> io::Result<()>,
) -> Result<()> {
    json_io::publish_new_with(path, value, LABEL, None, writer, sync)
}
