use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::{Value, json};

use super::super::export::transport::Transport;
use super::super::s3::{Configuration, Operation, Request};
use super::super::{digest, input::BUCKETS};
use super::input::TargetContext;
use crate::Result;

pub(crate) fn verify<T: Transport>(context: &TargetContext, transport: &T) -> Result<Value> {
    let initialized = context
        .initialized
        .get("objects")
        .and_then(Value::as_object)
        .ok_or("目标初始化收据缺少 objects")?;
    let scope = context.target_scope()?;
    let mut result = BTreeMap::new();
    for bucket in BUCKETS.iter().copied().collect::<BTreeSet<_>>() {
        object(context, transport, bucket, Operation::HeadBucket)?;
        let versioning = object(context, transport, bucket, Operation::Versioning)?;
        if versioning.get("Status").is_some() {
            return Err(format!("目标 {bucket} 启用了版本化，不能作为 fresh 目标").into());
        }
        let keys = list(context, transport, bucket, scope)?;
        let expected = initialized
            .get(bucket)
            .ok_or_else(|| format!("目标初始化收据缺少 {bucket} 对象状态"))?;
        let expected_keys = expected
            .get("keys")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("目标初始化收据 {bucket} 缺少 key 列表"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| "目标初始化对象 key 无效".into())
            })
            .collect::<Result<Vec<_>>>()?;
        if keys != expected_keys {
            return Err(format!("目标 {bucket} 对象 key 集合与初始化不同").into());
        }
        let owner_path = context.observation_dir.join(format!("owner-{bucket}.bin"));
        let owner = owner(context, transport, bucket, scope, &owner_path)?;
        if expected.get("owner") != Some(&owner) {
            return Err(format!("目标 {bucket} owner 字节或摘要与初始化不同").into());
        }
        result.insert(bucket.to_owned(), json!({"keys": keys, "owner": owner}));
    }
    Ok(Value::Object(result.into_iter().collect()))
}

fn list<T: Transport>(
    context: &TargetContext,
    transport: &T,
    bucket: &str,
    scope: &str,
) -> Result<Vec<String>> {
    let mut keys = BTreeSet::new();
    let mut token: Option<String> = None;
    let mut tokens = BTreeSet::new();
    let prefix = format!("{scope}/");
    for _ in 0..10_000 {
        let value = object(
            context,
            transport,
            bucket,
            Operation::List {
                prefix: prefix.clone(),
                continuation: token.clone(),
            },
        )?;
        if !matches!(value.get("IsTruncated"), Some(Value::Bool(_))) {
            return Err("目标对象分页缺少明确完成状态".into());
        }
        for item in value
            .get("Contents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let key = item
                .get("Key")
                .and_then(Value::as_str)
                .ok_or("目标对象 key 无效")?;
            if !key.starts_with(&prefix) || !keys.insert(key.to_owned()) {
                return Err("目标对象列表越界或重复".into());
            }
        }
        if value.get("IsTruncated").and_then(Value::as_bool) == Some(false) {
            return Ok(keys.into_iter().collect());
        }
        token = value
            .get("NextContinuationToken")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if token
            .as_ref()
            .is_none_or(|token| token.is_empty() || !tokens.insert(token.clone()))
        {
            return Err("目标对象分页缺少或重复 continuation token".into());
        }
    }
    Err("目标对象分页超过有界限制".into())
}

fn owner<T: Transport>(
    context: &TargetContext,
    transport: &T,
    bucket: &str,
    scope: &str,
    path: &Path,
) -> Result<Value> {
    object(
        context,
        transport,
        bucket,
        Operation::Get {
            key: format!("{scope}/.ryframe-owner"),
            if_match: None,
            version: None,
            destination: path.to_owned(),
            max_bytes: 4096,
        },
    )?;
    let expected = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
    let actual = digest::file(path)?;
    if fs::read(path)? != expected.as_bytes() {
        return Err(format!("目标 {bucket} owner 字节不符").into());
    }
    Ok(json!({"bytes": actual.bytes, "sha256": actual.sha256}))
}

fn object<T: Transport>(
    context: &TargetContext,
    transport: &T,
    bucket: &str,
    operation: Operation,
) -> Result<Value> {
    let configuration = Configuration::read(&context.target()["s3"])?;
    transport.s3(Request {
        configuration,
        bucket: bucket.to_owned(),
        operation,
    })
}
