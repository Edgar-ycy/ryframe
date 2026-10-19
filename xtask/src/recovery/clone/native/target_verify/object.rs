use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    time::Duration,
};

use serde_json::{Value, json};

use super::super::export::transport::{CommandSpec, StdoutTarget, Transport};
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
        aws(
            context,
            transport,
            vec![
                "s3api".into(),
                "head-bucket".into(),
                "--bucket".into(),
                bucket.into(),
            ],
            "head-bucket",
        )?
        .success("目标对象桶检查")?;
        let versioning = aws(
            context,
            transport,
            vec![
                "s3api".into(),
                "get-bucket-versioning".into(),
                "--bucket".into(),
                bucket.into(),
            ],
            "get-bucket-versioning",
        )?
        .success("目标对象版本检查")?;
        let versioning: Value = serde_json::from_slice(&versioning.stdout)?;
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
    let prefix = format!("{scope}/");
    for _ in 0..10_000 {
        let mut args = vec![
            "--no-paginate".into(),
            "s3api".into(),
            "list-objects-v2".into(),
            "--bucket".into(),
            bucket.into(),
            "--prefix".into(),
            prefix.clone(),
            "--max-keys".into(),
            "1000".into(),
        ];
        if let Some(token) = &token {
            args.extend(["--continuation-token".into(), token.clone()]);
        }
        let raw = aws(context, transport, args, "list-objects-v2")?.success("目标对象列表")?;
        let value: Value = serde_json::from_slice(&raw.stdout)?;
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
        if token.is_none() {
            return Err("目标对象分页缺少 continuation token".into());
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
    let mut args = vec![
        "s3api".into(),
        "get-object".into(),
        "--bucket".into(),
        bucket.into(),
        "--key".into(),
        format!("{scope}/.ryframe-owner"),
    ];
    let output = aws(
        context,
        transport,
        {
            args.push(path.display().to_string());
            args
        },
        "get-object-owner",
    )?;
    output.success("目标 owner 读取")?;
    let expected = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
    let actual = digest::file(path)?;
    if fs::read(path)? != expected.as_bytes() {
        return Err(format!("目标 {bucket} owner 字节不符").into());
    }
    Ok(json!({"bytes": actual.bytes, "sha256": actual.sha256}))
}

fn aws<T: Transport>(
    context: &TargetContext,
    transport: &T,
    args: Vec<String>,
    label: &str,
) -> Result<super::super::export::transport::CommandOutput> {
    let s3 = context.target().get("s3").ok_or("目标对象配置无效")?;
    let access = context
        .environment
        .get(
            s3["access_key_env"]
                .as_str()
                .ok_or("目标 access env 无效")?,
        )
        .ok_or("目标缺少对象访问凭据")?;
    let secret = context
        .environment
        .get(
            s3["secret_key_env"]
                .as_str()
                .ok_or("目标 secret env 无效")?,
        )
        .ok_or("目标缺少对象访问凭据")?;
    let mut environment = context.environment.clone();
    environment.retain(|key, _| !key.to_ascii_uppercase().starts_with("AWS_"));
    environment.insert("AWS_ACCESS_KEY_ID".into(), access.clone());
    environment.insert("AWS_SECRET_ACCESS_KEY".into(), secret.clone());
    environment.insert(
        "AWS_DEFAULT_REGION".into(),
        s3["region"].as_str().unwrap_or("").to_owned(),
    );
    environment.insert("AWS_EC2_METADATA_DISABLED".into(), "true".into());
    environment.insert("AWS_CONFIG_FILE".into(), dev_null());
    environment.insert("AWS_SHARED_CREDENTIALS_FILE".into(), dev_null());
    environment.insert("AWS_PAGER".into(), String::new());
    environment.insert("AWS_MAX_ATTEMPTS".into(), "1".into());
    let mut command = vec![
        "--endpoint-url".into(),
        s3["endpoint"].as_str().unwrap_or("").into(),
        "--region".into(),
        s3["region"].as_str().unwrap_or("").into(),
    ];
    command.extend(args);
    transport.run(CommandSpec {
        program: context.tool("aws")?,
        args: command,
        cwd: context.backend.clone(),
        environment,
        stdin: None,
        stdout: StdoutTarget::Capture,
        work_dir: context.observation_dir.clone(),
        label: label.to_owned(),
        timeout: Duration::from_secs(1800),
    })
}

fn dev_null() -> String {
    if cfg!(windows) {
        "NUL".into()
    } else {
        "/dev/null".into()
    }
}
