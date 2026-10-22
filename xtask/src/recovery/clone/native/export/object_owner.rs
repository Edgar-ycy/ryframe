use std::{fs, path::Path};

use serde_json::Value;

use super::super::{digest, json as json_io};
use super::{
    input::Context,
    object::{ObjectCall, object_read},
    transport::{StdoutTarget, Transport},
};
use crate::Result;

pub(super) fn owner_proofs<T: Transport>(
    context: &Context,
    transport: &T,
    directory: &Path,
    phase: &str,
) -> Result<Vec<Value>> {
    let scope = context.source()["scope_id"]
        .as_str()
        .ok_or("源 scope 无效")?;
    let mut result = Vec::new();
    let mut buckets = super::super::input::BUCKETS.to_vec();
    buckets.sort_unstable();
    for bucket in buckets {
        let file = format!("{phase}-{bucket}.bin");
        let body = directory.join(&file);
        let raw = object_read(
            context,
            transport,
            ObjectCall {
                operation: "get-object",
                bucket,
                key: &format!("{scope}/.ryframe-owner"),
                stdout: StdoutTarget::File(body.clone()),
                extra: None,
                label: &format!("{phase}-{bucket}"),
                max_bytes: 4096,
            },
        )?;
        let response = raw;
        write_stage_proof(
            directory,
            &format!("{phase}-{bucket}"),
            "get-object",
            &response,
        )?;
        let expected = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
        if fs::read(&body)? != expected.as_bytes() {
            return Err("对象 ownership 不属于本次明确隔离 scope".into());
        }
        result.push(serde_json::json!({
            "bucket": bucket,
            "key": format!("{scope}/.ryframe-owner"),
            "file": file,
            "bytes": digest::file(&body)?.bytes,
            "sha256": digest::file(&body)?.sha256,
        }));
    }
    Ok(result)
}

fn write_stage_proof(directory: &Path, stage: &str, operation: &str, raw: &Value) -> Result<()> {
    json_io::write_new(&directory.join(format!("{stage}.json")), raw)?;
    json_io::write_new(
        &directory.join(format!("{stage}.diagnostic.json")),
        &serde_json::json!({
            "operation": operation,
            "returncode": 0,
            "error_type": null,
            "stderr": "",
        }),
    )?;
    Ok(())
}
