use std::{
    fs::File,
    io::{self, BufWriter, Read, Write},
    path::Path,
};

use super::{
    super::{digest, resources::common},
    model,
};
use crate::{Result, recovery::monitoring::staging};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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

pub(super) struct HashingIo<R> {
    inner: R,
    hash: Sha256,
    bytes: u64,
}

impl<R> HashingIo<R> {
    pub(super) fn new(inner: R) -> Self {
        Self {
            inner,
            hash: Sha256::new(),
            bytes: 0,
        }
    }
    pub(super) fn finish(self) -> (R, Value) {
        (
            self.inner,
            json!({"bytes": self.bytes, "sha256": hex::encode(self.hash.finalize())}),
        )
    }
}

impl<R: Read> Read for HashingIo<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.hash.update(&buffer[..read]);
        self.bytes = self
            .bytes
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("SQL 流计数字节溢出"))?;
        Ok(read)
    }
}

impl<W: Write> Write for HashingIo<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.hash.update(&buffer[..written]);
        self.bytes = self
            .bytes
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::other("SQL 流写入字节数溢出"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
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
    if actual["bytes"] != written["bytes"]
        || actual["sha256"] != written["sha256"]
        || actual["device"] != device
        || actual["inode"] != inode
    {
        return Err("SQL 流结果与实际序列化写入字节或原文件身份不同".into());
    }
    Ok(actual)
}
