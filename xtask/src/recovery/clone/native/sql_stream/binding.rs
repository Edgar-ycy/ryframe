use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use super::{
    super::{
        digest::{self, HashingIo},
        resources::common,
    },
    model,
};
use crate::{Result, recovery::monitoring::staging};
use serde_json::Value;

pub(super) fn receipt(path: &Path, name: &str, relative: bool) -> Result<Value> {
    model::plain(path, false)?;
    let mut value = staging::file_receipt(path, name)?;
    if relative {
        let name = value.as_object_mut().unwrap().remove("path").unwrap();
        value["file"] = name;
    }
    Ok(value)
}

pub(super) fn open(path: &Path, expected: &Value) -> Result<File> {
    let key = if expected.get("file").is_some() {
        "file"
    } else {
        "path"
    };
    if receipt(path, model::text(&expected[key])?, key == "file")? != *expected {
        return Err("SQL 流文件内容或身份与绑定不同".into());
    }
    let file = File::open(path)?;
    check_handle(path, &file, expected)?;
    Ok(file)
}

pub(super) fn check_handle(path: &Path, file: &File, expected: &Value) -> Result<()> {
    model::plain(path, false)?;
    let metadata = file.metadata()?;
    let (device, inode) = common::device_inode_for(path, &metadata)?;
    if !metadata.is_file()
        || digest::linked(&metadata)
        || expected["bytes"] != metadata.len()
        || expected["device"] != device
        || expected["inode"] != inode
    {
        return Err("SQL 流打开句柄不属于原文件".into());
    }
    Ok(())
}

pub(super) fn finish_output(mut writer: BufWriter<HashingIo<File>>, path: &Path) -> Result<Value> {
    writer.flush()?;
    let (file, written) = writer
        .into_inner()
        .map_err(|error| error.into_error())?
        .finish();
    file.sync_all()?;
    let (device, inode) = common::device_inode_for(path, &file.metadata()?)?;
    drop(file);
    let actual = receipt(path, "rows.jsonl", true)?;
    if actual["bytes"] != written.bytes
        || actual["sha256"] != written.sha256
        || actual["device"] != device
        || actual["inode"] != inode
    {
        return Err("SQL 流结果与实际序列化写入字节或原文件身份不同".into());
    }
    Ok(actual)
}
