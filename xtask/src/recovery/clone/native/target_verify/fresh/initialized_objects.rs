//! reset后的完整对象owner观察；只消费固定scope，不覆盖远端对象。
use super::{
    super::{
        super::{
            digest, export::transport::Transport, input::BUCKETS, json as document, s3::Operation,
        },
        storage::StorageInputs,
    },
    objects,
};
use crate::Result;
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs};

pub(crate) fn observe(inputs: &StorageInputs<'_>, transport: &impl Transport) -> Result<Value> {
    let scope = document::text(&inputs.request["target"]["scope_id"], "对象scope")?;
    let key = format!("{scope}/.ryframe-owner");
    let mut result = serde_json::Map::new();
    for bucket in BUCKETS.iter().copied().collect::<BTreeSet<_>>() {
        objects::observe(
            inputs,
            transport,
            bucket,
            Operation::HeadBucket,
            "bucket-readiness",
            None,
        )?;
        let version = objects::observe(
            inputs,
            transport,
            bucket,
            Operation::Versioning,
            "bucket-versioning",
            None,
        )?;
        if !version["Status"].is_null() {
            return Err("fresh对象桶启用版本，不能接管".into());
        }
        if keys(inputs, transport, bucket, scope)? != BTreeSet::from([key.clone()]) {
            return Err("fresh对象范围包含未知对象或owner缺失".into());
        }
        let path = inputs.observation_dir.join(format!(
            "owner-{bucket}-{}.bin",
            uuid::Uuid::new_v4().simple()
        ));
        let raw = objects::observe(
            inputs,
            transport,
            bucket,
            Operation::Get {
                key: key.clone(),
                if_match: None,
                version: None,
                destination: path.clone(),
                max_bytes: 4096,
            },
            "owner-read",
            None,
        )?;
        let expected = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
        if fs::read(&path)? != expected.as_bytes() {
            return Err("fresh对象owner正文不同于固定scope".into());
        }
        metadata(&raw, expected.len())?;
        let digest = digest::file(&path)?;
        result.insert(
            bucket.into(),
            json!({"keys":[key],"owner":{"bytes":digest.bytes,"sha256":digest.sha256}}),
        );
    }
    Ok(Value::Object(result))
}

fn keys(
    inputs: &StorageInputs<'_>,
    transport: &impl Transport,
    bucket: &str,
    scope: &str,
) -> Result<BTreeSet<String>> {
    let prefix = format!("{scope}/");
    let mut token = None;
    let mut seen = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for _ in 0..10000 {
        let page = objects::observe(
            inputs,
            transport,
            bucket,
            Operation::List {
                prefix: prefix.clone(),
                continuation: token.clone(),
            },
            "scoped-list",
            Some(prefix.clone()),
        )?;
        let truncated = page["IsTruncated"]
            .as_bool()
            .ok_or("对象分页没有明确完成状态")?;
        for item in page
            .get("Contents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let key = document::text(&item["Key"], "对象key")?;
            if !key.starts_with(&prefix) || !keys.insert(key.to_owned()) {
                return Err("对象分页包含越界或重复key".into());
            }
        }
        if !truncated {
            return Ok(keys);
        }
        let next = document::text(&page["NextContinuationToken"], "对象分页token")?;
        if next.is_empty() || !seen.insert(next.to_owned()) {
            return Err("对象分页token缺失或循环".into());
        }
        token = Some(next.to_owned());
    }
    Err("对象分页超过有界限制".into())
}

pub(in super::super) fn metadata(value: &Value, bytes: usize) -> Result<()> {
    if value["ContentLength"].as_u64() != Some(bytes as u64)
        || ["ETag", "LastModified", "ContentType"]
            .iter()
            .any(|key| !value[*key].as_str().is_some_and(|v| !v.is_empty()))
        || !value["Metadata"].is_object()
    {
        return Err("迁移续作对象owner响应元数据无效".into());
    }
    Ok(())
}
