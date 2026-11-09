use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use super::{
    super::{digest, json as json_io},
    binding, model,
};
use crate::{Result, recovery::monitoring::execution::ProtectedInputs};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CHUNK: usize = 4 * 1024 * 1024;

struct Opened {
    root: PathBuf,
    rows: File,
    manifest: File,
    source: File,
    guard: ProtectedInputs,
}

impl Opened {
    fn open(stream: &Value) -> Result<Self> {
        let root = model::descriptor(stream)?;
        let source = model::path(&stream["source"]["path"], false)?;
        let guard = ProtectedInputs::open([
            source.clone(),
            root.join("manifest.json"),
            root.join("rows.jsonl"),
        ])?;
        let rows = binding::open(
            &digest::safe_file(&root, "rows.jsonl")?,
            &stream["artifact"],
        )?;
        let source_file = binding::open(&source, &stream["source"])?;
        let mut manifest = binding::open(
            &digest::safe_file(&root, "manifest.json")?,
            &stream["manifest"],
        )?;
        let mut bytes = Vec::new();
        (&mut manifest)
            .take(json_io::MAX_JSON_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > json_io::MAX_JSON_BYTES {
            return Err("SQL 流清单超过协议限制".into());
        }
        let ready = json_io::parse_fragment(std::str::from_utf8(&bytes)?, "SQL 流清单")?;
        let mut expected = stream.clone();
        expected.as_object_mut().unwrap().remove("manifest");
        expected["state"] = json!("ready");
        if ready != expected {
            return Err("SQL 流描述与原 ready 清单不同".into());
        }
        guard.assert_unchanged()?;
        Ok(Self {
            root,
            rows,
            manifest,
            source: source_file,
            guard,
        })
    }

    fn check(&self, stream: &Value) -> Result<()> {
        model::members(&self.root)?;
        binding::check_handle(
            &self.root.join("rows.jsonl"),
            &self.rows,
            &stream["artifact"],
        )?;
        binding::check_handle(
            &self.root.join("manifest.json"),
            &self.manifest,
            &stream["manifest"],
        )?;
        binding::check_handle(
            Path::new(model::text(&stream["source"]["path"])?),
            &self.source,
            &stream["source"],
        )?;
        self.guard.assert_unchanged()
    }
}

pub(super) fn read(args: &Value) -> Result<Value> {
    model::exact(args, &["stream", "offset"])?;
    let offset = args["offset"].as_u64().ok_or("SQL 流偏移无效")?;
    let stream = &args["stream"];
    let mut opened = Opened::open(stream)?;
    let result = chunk(&mut opened.rows, stream, offset);
    let result = match result {
        Ok(value) => opened.check(stream).map(|()| value),
        Err(error) => Err(error),
    };
    opened.guard.finish(result)
}

fn chunk(rows: &mut File, stream: &Value, offset: u64) -> Result<Value> {
    let size = stream["artifact"]["bytes"]
        .as_u64()
        .ok_or("SQL 流长度无效")?;
    if offset > size {
        return Err("SQL 流偏移越界".into());
    }
    rows.seek(SeekFrom::Start(offset))?;
    let mut buffer = Vec::with_capacity(CHUNK);
    rows.take(CHUNK as u64).read_to_end(&mut buffer)?;
    let valid = match std::str::from_utf8(&buffer) {
        Ok(_) => buffer.len(),
        Err(error) if error.error_len().is_none() && offset + (buffer.len() as u64) < size => {
            error.valid_up_to()
        }
        Err(_) => return Err("SQL 流结果不是完整 UTF-8".into()),
    };
    buffer.truncate(valid);
    if buffer.is_empty() && offset != size {
        return Err("SQL 流结果被截断或偏移不是字符边界".into());
    }
    let next = offset
        .checked_add(buffer.len() as u64)
        .ok_or("SQL 流偏移溢出")?;
    let hash = hex::encode(Sha256::digest(&buffer));
    let text = String::from_utf8(buffer)?;
    let value = json!({"offset": offset, "next_offset": next, "eof": next == size, "text": text, "chunk_sha256": hash});
    if serde_json::to_vec(&value)?.len() as u64 > json_io::MAX_JSON_BYTES - 1024 {
        return Err("SQL 流分块编码超过原协议限制".into());
    }
    Ok(value)
}

pub(super) fn close(args: &Value) -> Result<Value> {
    model::exact(args, &["stream"])?;
    let stream = &args["stream"];
    let opened = Opened::open(stream).map_err(|error| {
        format!("SQL 流关闭验证失败；原因={error}；rows.jsonl、manifest.json 均未删除，保留原 root，不自动重试")
    })?;
    opened
        .check(stream)
        .and_then(|()| opened.guard.finish(Ok(())))
        .map_err(|error| {
            format!("SQL 流关闭验证失败；原因={error}；rows.jsonl、manifest.json 均未删除，保留原 root，不自动重试")
        })?;
    let root = opened.root.clone();
    drop(opened);
    remove(&root, stream)?;
    Ok(json!(true))
}

fn remove(root: &Path, stream: &Value) -> Result<()> {
    // 释放保护句柄后再次核对固定成员；每一步只删除原绑定文件，失败立即保留剩余现场。
    for (deleted, (name, field)) in [("rows.jsonl", "artifact"), ("manifest.json", "manifest")]
        .into_iter()
        .enumerate()
    {
        let result: Result<()> = (|| {
            model::plain(root, true)?;
            if deleted == 0 {
                model::members(root)?;
            }
            let path = digest::safe_file(root, name)?;
            if binding::receipt(&path, name, true)? != stream[field] {
                return Err("SQL 流待删除文件的原绑定变化".into());
            }
            fs::remove_file(path)?;
            Ok(())
        })();
        result.map_err(|error| {
            let code = error.downcast_ref::<std::io::Error>().and_then(std::io::Error::raw_os_error);
            format!("SQL 流本地关闭失败；已删除 {deleted} 个固定文件，当前 {name}；原因={error}；OS {code:?}；保留 root {}，不自动重试", root.display())
        })?;
    }
    fs::remove_dir(root).map_err(|error| format!("SQL 流本地关闭失败；rows.jsonl、manifest.json 已删除，空目录保留 {}；OS {:?}；不自动重试", root.display(), error.raw_os_error()))?;
    Ok(())
}
