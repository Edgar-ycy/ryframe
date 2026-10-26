//! 完整像只重读既有对象证据；不向 S3 请求，不赋予当前资源或复制成功资格。
use std::{collections::BTreeMap, fs, path::Path};

use chrono::{DateTime, SecondsFormat};
use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        digest, input::BUCKETS, json as document, metadata, resources::common,
        s3::app_configuration, target_verify::guard::Inputs,
    },
};

const COPY: &[&str] = &[
    "CacheControl",
    "ContentDisposition",
    "ContentEncoding",
    "ContentLanguage",
    "ContentType",
    "Expires",
    "Metadata",
    "WebsiteRedirectLocation",
];
const OBSERVED: &[&str] = &[
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

#[path = "objects/capture.rs"]
pub(super) mod capture;

pub(super) fn owner(
    backend: &Path,
    directory: &Path,
    phase: &str,
    bucket: &str,
    scope: &str,
) -> Result<Value> {
    let filename = format!("{phase}-{bucket}.bin");
    let path = common::common_file(
        backend,
        &directory.join(&filename),
        "对象 ownership 原始字节",
    )?;
    let expected = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
    if fs::metadata(&path)?.len() != expected.len() as u64
        || fs::read(&path)? != expected.as_bytes()
    {
        return Err("对象 ownership 不属于明确计划 scope".into());
    }
    let actual = digest::file(&path)?;
    Ok(
        json!({"bucket":bucket,"key":format!("{scope}/.ryframe-owner"),"file":filename,
        "bytes":actual.bytes,"sha256":actual.sha256}),
    )
}

pub(super) fn verify(
    backend: &Path,
    selected: &Value,
    directory: &Path,
    expected: &Value,
    max_bytes: u64,
) -> Result<Value> {
    common::local_directory(backend, directory, "完整像对象证据")?;
    let value = document::read(&directory.join("capture.json"))?;
    let bucket = document::text(&value["bucket"], "对象桶")?;
    let key = document::text(&value["key"], "对象键")?;
    let scope = document::text(&selected["scope_id"], "对象 scope")?;
    if !BUCKETS.contains(&bucket) || !key.starts_with(&format!("{scope}/")) {
        return Err("对象证据不属于明确五桶和来源 scope".into());
    }
    let files = evidence_names();
    let held = Inputs::open(files.iter().map(|name| directory.join(name)))?;
    reject_failure(directory)?;
    let configuration = &selected["s3"]["configuration"];
    let objects = app_configuration::public_configuration(backend, configuration)?;
    let request = json!({"format_version":1,"kind":"devex-clone-object-capture","side":"source",
        "scope_id":scope,"endpoint":objects["endpoint"],"region":objects["region"],"bucket":bucket,
        "key":key,"expected":expected,"max_bytes":max_bytes,"transport":"rust-s3-sigv4","clone_verified":false});
    if document::read(&directory.join("intent.json"))? != request {
        return Err("对象采集 intent 与明确请求、文件配置或目标不同".into());
    }
    let (before, after) = owners(backend, directory, scope)?;
    let head = document::read(&directory.join("head-before.json"))?;
    let get = document::read(&directory.join("get.json"))?;
    let repeated = document::read(&directory.join("head-after.json"))?;
    let actual = digest::file(&directory.join("object.bin"))?;
    let calculated = result(
        &request,
        &head,
        &get,
        &repeated,
        &json!({"bytes":actual.bytes,"sha256":actual.sha256}),
        before,
        after,
    )?;
    if value != calculated {
        return Err("对象采集结果与原始 HEAD、GET、owner 或实际文件重算不符".into());
    }
    held.check()?;
    app_configuration::public_configuration(backend, configuration)?;
    reject_failure(directory)?;
    Ok(calculated)
}

fn evidence_names() -> Vec<String> {
    let mut names = vec![
        "intent.json".into(),
        "object.bin".into(),
        "capture.json".into(),
    ];
    for stage in ["head-before", "get", "head-after"]
        .into_iter()
        .map(str::to_owned)
        .chain(
            ["owner-before", "owner-after"]
                .into_iter()
                .flat_map(|phase| {
                    BUCKETS
                        .iter()
                        .map(move |bucket| format!("{phase}-{bucket}"))
                }),
        )
    {
        names.push(format!("{stage}.json"));
        names.push(format!("{stage}.diagnostic.json"));
        if stage.starts_with("owner-") {
            names.push(format!("{stage}.bin"));
        }
    }
    names
}

fn owners(backend: &Path, directory: &Path, scope: &str) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut stages = BTreeMap::from([
        ("head-before".to_owned(), "head-object"),
        ("get".to_owned(), "get-object"),
        ("head-after".to_owned(), "head-object"),
    ]);
    let mut rows = Vec::new();
    let mut buckets = BUCKETS.to_vec();
    buckets.sort_unstable();
    for phase in ["owner-before", "owner-after"] {
        let mut owners = Vec::new();
        for bucket in &buckets {
            owners.push(owner(backend, directory, phase, bucket, scope)?);
            stages.insert(format!("{phase}-{bucket}"), "get-object");
        }
        rows.push(owners);
    }
    for (stage, operation) in stages {
        let diagnostic = document::read(&directory.join(format!("{stage}.diagnostic.json")))?;
        document::exact(
            &diagnostic,
            &["operation", "returncode", "error_type", "stderr"],
            "对象只读请求诊断",
        )?;
        if diagnostic["operation"] != operation
            || diagnostic["returncode"].as_i64() != Some(0)
            || !diagnostic["error_type"].is_null()
            || !diagnostic["stderr"].is_string()
        {
            return Err("对象只读请求诊断不完整或包含失败".into());
        }
        document::object(
            &document::read(&directory.join(format!("{stage}.json")))?,
            "对象原始响应",
        )?;
    }
    let after = rows.pop().ok_or("对象后 ownership 缺失")?;
    let before = rows.pop().ok_or("对象前 ownership 缺失")?;
    Ok((before, after))
}

fn inspect(value: &Value) -> Result<Value> {
    let fields = document::object(value, "对象响应")?;
    if fields
        .keys()
        .any(|name| !COPY.contains(&name.as_str()) && !OBSERVED.contains(&name.as_str()))
        || !fields.contains_key("ContentType")
        || !fields.contains_key("Metadata")
        || fields
            .get("MissingMeta")
            .is_some_and(|v| v.as_u64() != Some(0))
        || fields
            .get("TagCount")
            .is_some_and(|v| v.as_u64() != Some(0))
        || fields.get("StorageClass").is_some_and(|v| v != "STANDARD")
        || fields.get("AcceptRanges").is_some_and(|v| v != "bytes")
        || value["ContentLength"].as_u64().is_none()
    {
        return Err("对象响应元数据缺失、截断或含不支持策略".into());
    }
    let etag = document::text(&value["ETag"], "对象 ETag")?;
    let body = etag
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .filter(|s| !s.is_empty())
        .ok_or("对象 ETag 无效")?;
    if !body
        .bytes()
        .all(|byte| (0x21..=0x7e).contains(&byte) && byte != b'"')
    {
        return Err("对象 ETag 无效".into());
    }
    if let Some(version) = fields.get("VersionId") {
        let text = document::text(version, "对象版本")?;
        if text.is_empty() || text.chars().any(|character| (character as u32) < 32) {
            return Err("对象版本标识无效".into());
        }
    }
    let time =
        DateTime::parse_from_rfc3339(document::text(&value["LastModified"], "对象修改时间")?)
            .map_err(|_| "对象修改时间必须带明确时区")?
            .to_utc();
    let precision = if time.timestamp_subsec_micros() == 0 {
        SecondsFormat::Secs
    } else {
        SecondsFormat::Micros
    };
    let copy = COPY
        .iter()
        .map(|name| ((*name).to_owned(), value[*name].clone()))
        .collect();
    Ok(
        json!({"metadata":metadata::copy(&Value::Object(copy))?,"identity":{
        "ContentLength":value["ContentLength"],"ETag":etag,"LastModified":time.to_rfc3339_opts(precision,false),
        "VersionId":value.get("VersionId").cloned().unwrap_or(Value::Null)}}),
    )
}

fn result(
    request: &Value,
    head: &Value,
    get: &Value,
    repeated: &Value,
    actual: &Value,
    before: Vec<Value>,
    after: Vec<Value>,
) -> Result<Value> {
    let first = inspect(head)?;
    let downloaded = inspect(get)?;
    let last = inspect(repeated)?;
    if first["identity"] != downloaded["identity"]
        || *actual != request["expected"]
        || actual["bytes"] != first["identity"]["ContentLength"]
        || actual["bytes"]
            .as_u64()
            .zip(request["max_bytes"].as_u64())
            .is_none_or(|(bytes, max)| bytes > max)
        || first != last
        || head != repeated
    {
        return Err("对象身份、完整字节、摘要或采集前后 HEAD 不同".into());
    }
    let differences = COPY.iter().filter(|field| head[**field] != get[**field])
        .map(|field| json!({"field":field,"head_present":head.get(*field).is_some(),"head_value":head[*field],
            "get_present":get.get(*field).is_some(),"get_value":get[*field]})).collect::<Vec<_>>();
    let mut value = request.clone();
    let fields = json!({"status":"captured","stored_metadata_verified":true,"bytes_verified":true,
        "get_header_consistent":differences.is_empty(),"get_header_differences":differences,
        "metadata":first["metadata"],"identity":first["identity"],"response_sha256":{
            "head_before":digest::plan_hash(head)?,"get":digest::plan_hash(get)?,"head_after":digest::plan_hash(repeated)?},
        "artifact":{"file":"object.bin","bytes":actual["bytes"],"sha256":actual["sha256"]},
        "ownership_before":before,"ownership_after":after,"request_sha256":digest::plan_hash(request)?,
        "live_preconditions":"仅证明本次精确对象观察；源停止、完整清单和实际复制仍须上层独立验证"});
    value
        .as_object_mut()
        .ok_or("对象请求必须是对象")?
        .extend(document::object(&fields, "对象采集结果")?.clone());
    Ok(value)
}

fn reject_failure(directory: &Path) -> Result<()> {
    if directory.join("failure.json").try_exists()? {
        return Err("对象采集目录有失败证据，不能使用残留成功文件".into());
    }
    Ok(())
}
