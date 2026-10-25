use std::{collections::BTreeMap, fs, path::Path};

use chrono::DateTime;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::{digest, input::BUCKETS, json as json_io, json::exact, metadata};
use super::{
    input::Context,
    object_owner::owner_proofs,
    transport::{StdoutTarget, Transport},
};
use crate::Result;

pub(crate) type ObjectIndex = BTreeMap<(String, String), Value>;

pub(super) struct ObjectCall<'a> {
    pub(super) operation: &'a str,
    pub(super) bucket: &'a str,
    pub(super) key: &'a str,
    pub(super) stdout: StdoutTarget,
    pub(super) extra: Option<&'a [String]>,
    pub(super) label: &'a str,
    pub(super) max_bytes: u64,
}

pub(crate) fn verify_owners<T: Transport>(
    context: &Context,
    transport: &T,
    phase: &str,
) -> Result<()> {
    let scope = context.source()["scope_id"]
        .as_str()
        .ok_or("源 scope 无效")?;
    for bucket in BUCKETS {
        let output = context.output().join(format!("{phase}-{bucket}.bin"));
        object_read(
            context,
            transport,
            ObjectCall {
                operation: "get-object",
                bucket,
                key: &format!("{scope}/.ryframe-owner"),
                stdout: StdoutTarget::File(output.clone()),
                extra: None,
                label: &format!("owner-{phase}-{bucket}"),
                max_bytes: 4096,
            },
        )?;
        let expected = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
        if fs::read(&output)? != expected.as_bytes() {
            return Err("对象 ownership 不属于本次明确隔离 scope".into());
        }
    }
    Ok(())
}

pub(crate) fn capture<T: Transport>(
    context: &Context,
    transport: &T,
    inventory: &Value,
    max_bytes: u64,
) -> Result<(Vec<Value>, ObjectIndex)> {
    let objects = context.output().join("objects");
    fs::create_dir(&objects).map_err(|error| format!("无法创建对象证据目录：{error}"))?;
    let buckets = inventory
        .get("objects")
        .and_then(Value::as_array)
        .ok_or("源库存缺少对象桶")?;
    let mut rows = Vec::new();
    let mut index = BTreeMap::new();
    for bucket in buckets {
        let bucket_name = bucket
            .get("bucket")
            .and_then(Value::as_str)
            .ok_or("对象桶无效")?;
        let entries = bucket
            .get("entries")
            .and_then(Value::as_array)
            .ok_or("对象桶缺少条目")?;
        let mut captured = Vec::new();
        for entry in entries {
            let entry = exact(entry, &["key", "bytes", "sha256"], "对象库存条目")?;
            let key = entry
                .get("key")
                .and_then(Value::as_str)
                .ok_or("对象 key 无效")?;
            let expected = json!({
                "bytes": entry["bytes"],
                "sha256": entry["sha256"],
            });
            let token = hex::encode(Sha256::digest(format!("{bucket_name}:{key}").as_bytes()));
            let directory = objects.join(&token);
            fs::create_dir(&directory)?;
            let value = capture_one(
                context,
                transport,
                bucket_name,
                key,
                &directory,
                &expected,
                max_bytes,
            )?;
            let relative = Path::new("objects").join(&token).join("object.bin");
            let capture_relative = Path::new("objects").join(&token).join("capture.json");
            let artifact = json!({
                "file": relative.to_string_lossy().replace('\\', "/"),
                "bytes": value["artifact"]["bytes"],
                "sha256": value["artifact"]["sha256"],
            });
            let capture_binding = json!({
                "file": capture_relative.to_string_lossy().replace('\\', "/"),
                "bytes": digest::file(&context.output().join(&capture_relative))?.bytes,
                "sha256": digest::file(&context.output().join(&capture_relative))?.sha256,
            });
            captured.push(json!({
                "key": key,
                "artifact": artifact,
                "metadata": value["metadata"],
                "capture": capture_binding,
                "get_header_consistent": value["get_header_consistent"],
            }));
            index.insert(
                (
                    bucket_name.to_owned(),
                    key.strip_prefix(&format!(
                        "{}/",
                        context.source()["scope_id"].as_str().unwrap_or_default()
                    ))
                    .unwrap_or(key)
                    .to_owned(),
                ),
                json!({
                    "bytes": entry["bytes"],
                    "sha256": entry["sha256"],
                }),
            );
        }
        rows.push(json!({"bucket": bucket_name, "entries": captured}));
    }
    Ok((rows, index))
}

fn capture_one<T: Transport>(
    context: &Context,
    transport: &T,
    bucket: &str,
    key: &str,
    directory: &Path,
    expected: &Value,
    max_bytes: u64,
) -> Result<Value> {
    let reads = read_object(context, transport, bucket, key, directory, expected)?;
    if reads.actual.bytes > max_bytes {
        return Err("GET 对象完整字节数超过采集上限".into());
    }
    let request = object_request(context, bucket, key, expected, max_bytes);
    json_io::write_new(&directory.join("intent.json"), &request)?;
    let ownership_before = owner_proofs(context, transport, directory, "owner-before")?;
    let ownership_after = owner_proofs(context, transport, directory, "owner-after")?;
    let proof = CaptureProof {
        request,
        reads,
        ownership_before,
        ownership_after,
    };
    let value = capture_value(context, bucket, key, expected, max_bytes, proof)?;
    json_io::write_new(&directory.join("capture.json"), &value)?;
    Ok(value)
}

struct ObjectReads {
    head_before: Value,
    head_after: Value,
    get: Value,
    actual: digest::FileDigest,
}

struct CaptureProof {
    request: Value,
    reads: ObjectReads,
    ownership_before: Vec<Value>,
    ownership_after: Vec<Value>,
}

fn read_object<T: Transport>(
    context: &Context,
    transport: &T,
    bucket: &str,
    key: &str,
    directory: &Path,
    expected: &Value,
) -> Result<ObjectReads> {
    let body = directory.join("object.bin");
    let head_before = object_json(
        context,
        transport,
        "head-object",
        bucket,
        key,
        &[],
        &format!("head-before-{}", safe_key(key)),
    )?;
    validate_head(&head_before, expected)?;
    write_stage_proof(directory, "head-before", "head-object", &head_before)?;
    let mut identity_args = vec![
        "--if-match".into(),
        head_before["ETag"]
            .as_str()
            .ok_or("对象 HEAD 缺少 ETag")?
            .to_owned(),
    ];
    if let Some(version) = head_before.get("VersionId").and_then(Value::as_str) {
        identity_args.extend(["--version-id".into(), version.to_owned()]);
    }
    let get = object_read(
        context,
        transport,
        ObjectCall {
            operation: "get-object",
            bucket,
            key,
            stdout: StdoutTarget::File(body.clone()),
            extra: Some(&identity_args),
            label: &format!("get-body-{}", safe_key(key)),
            max_bytes: expected["bytes"].as_u64().ok_or("对象库存缺少长度")?,
        },
    )?;
    let get_json = get;
    validate_response(&get_json)?;
    if !get_json.is_object() {
        return Err("对象 GET 响应不是 JSON 对象".into());
    }
    write_stage_proof(directory, "get", "get-object", &get_json)?;
    ensure_same_identity(&head_before, &get_json)?;
    let head_after = object_json(
        context,
        transport,
        "head-object",
        bucket,
        key,
        &identity_args,
        &format!("head-after-{}", safe_key(key)),
    )?;
    write_stage_proof(directory, "head-after", "head-object", &head_after)?;
    if head_after != head_before {
        return Err("采集前后对象 HEAD 或完整存储元数据变化".into());
    }
    let actual = digest::file(&body)?;
    if actual.bytes != expected["bytes"].as_u64().unwrap_or(u64::MAX)
        || actual.sha256 != expected["sha256"].as_str().unwrap_or_default()
    {
        return Err("GET 对象身份、完整字节数或 SHA256 与登记值不符".into());
    }
    Ok(ObjectReads {
        head_before,
        head_after,
        get: get_json,
        actual,
    })
}

fn object_request(
    context: &Context,
    bucket: &str,
    key: &str,
    expected: &Value,
    max_bytes: u64,
) -> Value {
    json!({
        "format_version": 1,
        "kind": "devex-clone-object-capture",
        "side": "source",
        "scope_id": context.source()["scope_id"],
        "endpoint": context.object_identity["endpoint"],
        "region": context.object_identity["region"],
        "bucket": bucket,
        "key": key,
        "expected": expected,
        "max_bytes": max_bytes,
        "transport": "rust-s3-sigv4",
        "clone_verified": false,
    })
}

fn capture_value(
    context: &Context,
    bucket: &str,
    key: &str,
    expected: &Value,
    max_bytes: u64,
    proof: CaptureProof,
) -> Result<Value> {
    let CaptureProof {
        request,
        reads,
        ownership_before,
        ownership_after,
    } = proof;
    let metadata = metadata::copy(&json!({
        "ContentType": reads.head_before.get("ContentType").cloned().unwrap_or(Value::Null),
        "CacheControl": reads.head_before.get("CacheControl").cloned().unwrap_or(Value::Null),
        "ContentDisposition": reads.head_before.get("ContentDisposition").cloned().unwrap_or(Value::Null),
        "ContentEncoding": reads.head_before.get("ContentEncoding").cloned().unwrap_or(Value::Null),
        "ContentLanguage": reads.head_before.get("ContentLanguage").cloned().unwrap_or(Value::Null),
        "Expires": reads.head_before.get("Expires").cloned().unwrap_or(Value::Null),
        "WebsiteRedirectLocation": reads.head_before.get("WebsiteRedirectLocation").cloned().unwrap_or(Value::Null),
        "Metadata": reads.head_before.get("Metadata").cloned().unwrap_or_else(|| json!({})),
    }))?;
    let consistent = header_fields(&reads.head_before) == header_fields(&reads.get);
    Ok(json!({
        "format_version": 1,
        "kind": "devex-clone-object-capture",
        "side": "source",
        "status": "captured",
        "bucket": bucket,
        "key": key,
        "scope_id": context.source()["scope_id"],
        "endpoint": context.object_identity["endpoint"],
        "region": context.object_identity["region"],
        "expected": expected,
        "max_bytes": max_bytes,
        "transport": "rust-s3-sigv4",
        "clone_verified": false,
        "stored_metadata_verified": true,
        "bytes_verified": true,
        "metadata": metadata,
        "identity": {
            "ContentLength": reads.head_before["ContentLength"],
            "ETag": reads.head_before["ETag"],
            "LastModified": normalized_time(&reads.head_before["LastModified"])?,
            "VersionId": reads.head_before.get("VersionId").cloned().unwrap_or(Value::Null),
        },
        "response_sha256": {
            "head_before": digest::plan_hash(&reads.head_before)?,
            "get": digest::plan_hash(&reads.get)?,
            "head_after": digest::plan_hash(&reads.head_after)?,
        },
        "artifact": {"file": "object.bin", "bytes": reads.actual.bytes, "sha256": reads.actual.sha256},
        "get_header_consistent": consistent,
        "get_header_differences": differences(&reads.head_before, &reads.get),
        "ownership_before": ownership_before,
        "ownership_after": ownership_after,
        "request_sha256": digest::plan_hash(&request)?,
        "live_preconditions": "仅证明本次精确对象观察；源停止、完整清单和实际复制仍须上层独立验证",
    }))
}

fn write_stage_proof(directory: &Path, stage: &str, operation: &str, raw: &Value) -> Result<()> {
    json_io::write_new(&directory.join(format!("{stage}.json")), raw)?;
    json_io::write_new(
        &directory.join(format!("{stage}.diagnostic.json")),
        &json!({
            "operation": operation,
            "returncode": 0,
            "error_type": null,
            "stderr": "",
        }),
    )?;
    Ok(())
}

fn validate_head(value: &Value, expected: &Value) -> Result<()> {
    validate_response(value)?;
    let length = value
        .get("ContentLength")
        .and_then(Value::as_u64)
        .ok_or("对象 HEAD 缺少 ContentLength")?;
    if Some(length) != expected.get("bytes").and_then(Value::as_u64)
        || value.get("ETag").and_then(Value::as_str).is_none()
        || value.get("LastModified").and_then(Value::as_str).is_none()
        || value.get("ContentType").and_then(Value::as_str).is_none()
        || value.get("Metadata").and_then(Value::as_object).is_none()
    {
        return Err("对象 HEAD 大小、ETag、修改时间、ContentType 或 Metadata 无效".into());
    }
    if value
        .get("MissingMeta")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        != 0
        || value.get("TagCount").and_then(Value::as_u64).unwrap_or(0) != 0
        || value
            .get("StorageClass")
            .and_then(Value::as_str)
            .unwrap_or("STANDARD")
            != "STANDARD"
    {
        return Err("对象 HEAD 含截断、标签或非 STANDARD 存储策略".into());
    }
    Ok(())
}

fn validate_response(value: &Value) -> Result<()> {
    let allowed = [
        "ContentType",
        "CacheControl",
        "ContentDisposition",
        "ContentEncoding",
        "ContentLanguage",
        "Expires",
        "WebsiteRedirectLocation",
        "Metadata",
        "AcceptRanges",
        "ContentLength",
        "ETag",
        "LastModified",
        "VersionId",
        "StorageClass",
        "MissingMeta",
        "TagCount",
        "PartsCount",
        "ChecksumCRC32",
        "ChecksumCRC32C",
        "ChecksumCRC64NVME",
        "ChecksumSHA1",
        "ChecksumSHA256",
        "ChecksumType",
    ];
    if value
        .as_object()
        .is_none_or(|object| object.keys().any(|key| !allowed.contains(&key.as_str())))
        || value
            .get("AcceptRanges")
            .and_then(Value::as_str)
            .is_some_and(|value| value != "bytes")
    {
        return Err("对象 HEAD 含不支持的对象策略或非完整内容响应".into());
    }
    Ok(())
}

fn ensure_same_identity(head: &Value, get: &Value) -> Result<()> {
    if head["ContentLength"] != get["ContentLength"]
        || head["ETag"] != get["ETag"]
        || normalized_time(&head["LastModified"])? != normalized_time(&get["LastModified"])?
        || head.get("VersionId") != get.get("VersionId")
    {
        return Err("GET 对象身份与采集前 HEAD 不一致".into());
    }
    Ok(())
}

fn normalized_time(value: &Value) -> Result<String> {
    let value = value.as_str().ok_or("对象 LastModified 必须为字符串")?;
    Ok(DateTime::parse_from_rfc3339(value)?.to_utc().to_rfc3339())
}

fn header_fields(value: &Value) -> BTreeMap<String, Value> {
    [
        "ContentType",
        "CacheControl",
        "ContentDisposition",
        "ContentEncoding",
        "ContentLanguage",
        "Expires",
        "WebsiteRedirectLocation",
        "Metadata",
    ]
    .into_iter()
    .map(|field| {
        (
            field.to_owned(),
            value.get(field).cloned().unwrap_or(Value::Null),
        )
    })
    .collect()
}

fn differences(left: &Value, right: &Value) -> Vec<Value> {
    header_fields(left)
        .into_iter()
        .filter(|(field, value)| header_fields(right).get(field) != Some(value))
        .map(|(field, value)| {
            json!({
                "field": field,
                "head_present": !value.is_null(),
                "head_value": value,
                "get_present": !right.get(&field).unwrap_or(&Value::Null).is_null(),
                "get_value": right.get(&field).cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn object_json<T: Transport>(
    context: &Context,
    transport: &T,
    operation: &str,
    bucket: &str,
    key: &str,
    extra: &[String],
    label: &str,
) -> Result<Value> {
    let output = object_read(
        context,
        transport,
        ObjectCall {
            operation,
            bucket,
            key,
            stdout: StdoutTarget::Capture,
            extra: Some(extra),
            label,
            max_bytes: 0,
        },
    )?;
    let value = output;
    if !value.is_object() {
        return Err("对象工具响应不是 JSON 对象".into());
    }
    Ok(value)
}

pub(super) fn object_read<T: Transport>(
    context: &Context,
    transport: &T,
    call: ObjectCall<'_>,
) -> Result<Value> {
    use super::super::s3::{Configuration, Operation, Request};
    let configuration = Configuration::from_app_binding(
        &context.backend,
        &context.source()["s3"]["configuration"],
    )?;
    let mut if_match = None;
    let mut version = None;
    if let Some(extra) = call.extra {
        let (pairs, remainder) = extra.as_chunks::<2>();
        if !remainder.is_empty() {
            return Err("对象读取条件不完整".into());
        }
        for pair in pairs {
            match pair[0].as_str() {
                "--if-match" if if_match.is_none() => if_match = Some(pair[1].clone()),
                "--version-id" if version.is_none() => version = Some(pair[1].clone()),
                _ => return Err("对象读取包含未知或重复条件".into()),
            }
        }
    }
    let operation = match (call.operation, call.stdout) {
        ("head-object", StdoutTarget::Capture) => Operation::Head {
            key: call.key.to_owned(),
            if_match,
            version,
        },
        ("get-object", StdoutTarget::File(destination)) => Operation::Get {
            key: call.key.to_owned(),
            if_match,
            version,
            destination,
            max_bytes: call.max_bytes,
        },
        _ => return Err("原生对象读取操作与输出方式不匹配".into()),
    };
    let result: Result<Value> = transport
        .s3(Request {
            configuration,
            bucket: call.bucket.to_owned(),
            operation,
        })
        .map_err(|error| format!("{}：{error}", call.label).into());
    Configuration::from_app_binding(&context.backend, &context.source()["s3"]["configuration"])?;
    result
}

fn safe_key(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
