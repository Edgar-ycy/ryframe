//! 原生 S3 条件写的私有标准输入协议；凭据不进入参数、环境或输出。

use std::io::{Read, Write};

use serde_json::{Value, json};

use super::{Configuration, Operation, PutRequest, Request};
use crate::Result;

const PUT_COMMAND: &str = "__s3-put-if-absent";
const CREATE_BUCKET_COMMAND: &str = "__s3-create-bucket";
const READ_COMMAND: &str = "__s3-read";
const INSPECT_COMMAND: &str = "__s3-inspect";
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let command = arguments.first().map(String::as_str);
    if !matches!(
        command,
        Some(PUT_COMMAND | CREATE_BUCKET_COMMAND | READ_COMMAND | INSPECT_COMMAND)
    ) {
        return None;
    }
    let result = if arguments.len() == 1 {
        read_request().and_then(|value| match command {
            Some(PUT_COMMAND) => execute_request(value),
            Some(CREATE_BUCKET_COMMAND) => execute_create_bucket_request(value),
            Some(READ_COMMAND) => execute_read_request(value),
            Some(INSPECT_COMMAND) => execute_inspect_request(value),
            _ => unreachable!(),
        })
    } else {
        Err("原生对象私有协议不接受命令行参数；请求必须通过标准输入传递".into())
    };
    let category = match command {
        Some(PUT_COMMAND) => "NativeS3PutRejected",
        Some(CREATE_BUCKET_COMMAND) => "NativeS3CreateBucketRejected",
        Some(READ_COMMAND) => "NativeS3ReadRejected",
        _ => "NativeS3InspectRejected",
    };
    let (code, response) = match result {
        Ok(value) => (0, json!({"ok": true, "result": value})),
        Err(error) => (
            1,
            json!({
                "ok": false,
                "error": {
                    "category": category,
                    "message": error.to_string(),
                    "remote_write_may_have_occurred": false,
                    "automatic_retry_permitted": false,
                }
            }),
        ),
    };
    let output = (|| -> Result<()> {
        let mut encoded = serde_json::to_vec(&response)?;
        encoded.push(b'\n');
        std::io::stdout().lock().write_all(&encoded)?;
        Ok(())
    })();
    Some(if output.is_ok() { code } else { 1 })
}

pub(super) fn execute_create_bucket_request(value: Value) -> Result<Value> {
    let request = super::super::json::exact(
        &value,
        &[
            "format_version",
            "kind",
            "scope_id",
            "configuration",
            "bucket",
        ],
        "原生对象桶创建请求",
    )?;
    if request.get("format_version").and_then(Value::as_u64) != Some(1)
        || request.get("kind").and_then(Value::as_str) != Some("ryframe-native-s3-create-bucket")
    {
        return Err("原生对象桶创建请求版本或类型无效".into());
    }
    let scope = required_text(request.get("scope_id"), "创建 scope")?;
    super::super::input::name(scope)?;
    let bucket = required_text(request.get("bucket"), "创建桶")?;
    if !super::super::input::BUCKETS.contains(&bucket) {
        return Err("原生对象桶创建只能访问已登记的业务对象桶".into());
    }
    super::create_bucket(
        Configuration::read(request.get("configuration").ok_or("创建桶缺少对象配置")?)?,
        bucket.to_owned(),
    )
}

fn read_request() -> Result<Value> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_REQUEST_BYTES {
        return Err("原生对象私有协议请求为空或超过 1 MiB".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "原生对象私有协议请求必须是 UTF-8 JSON")?;
    super::super::json::parse_fragment(text, "原生对象私有协议请求")
}

pub(super) fn execute_request(value: Value) -> Result<Value> {
    let request = super::super::json::exact(
        &value,
        &[
            "format_version",
            "kind",
            "scope_id",
            "configuration",
            "bucket",
            "key",
            "source",
            "expected",
            "metadata",
        ],
        "原生对象条件创建请求",
    )?;
    if request.get("format_version").and_then(Value::as_u64) != Some(1)
        || request.get("kind").and_then(Value::as_str) != Some("ryframe-native-s3-put-if-absent")
    {
        return Err("原生对象条件创建请求版本或类型无效".into());
    }
    let scope = required_text(request.get("scope_id"), "目标 scope")?;
    super::super::input::name(scope)?;
    let bucket = required_text(request.get("bucket"), "目标桶")?;
    if !super::super::input::BUCKETS.contains(&bucket) {
        return Err("条件创建只能写入已登记的业务对象桶".into());
    }
    let key = required_text(request.get("key"), "目标 key")?;
    validate_key(scope, key, false)?;
    let configuration =
        Configuration::read(request.get("configuration").ok_or("条件创建缺少对象配置")?)?;
    let source = required_text(request.get("source"), "来源文件")?;
    let source = crate::local_test_path::validate_local_test_path(
        std::path::Path::new(source),
        &crate::workspace::root_dir(),
        crate::local_test_path::LocalTestPathKind::ExistingFile,
    )
    .map_err(|error| format!("条件创建来源文件无效：{error}"))?;
    let expected = super::super::json::exact(
        request.get("expected").ok_or("条件创建缺少来源摘要")?,
        &["bytes", "sha256"],
        "条件创建来源摘要",
    )?;
    let expected_bytes = expected
        .get("bytes")
        .and_then(Value::as_u64)
        .ok_or("条件创建来源字节数无效")?;
    let expected_sha256 = required_text(expected.get("sha256"), "来源 SHA256")?.to_owned();
    let result = super::put_if_absent(PutRequest {
        configuration,
        bucket: bucket.to_owned(),
        key: key.to_owned(),
        source,
        expected_bytes,
        expected_sha256,
        metadata: request
            .get("metadata")
            .ok_or("条件创建缺少对象元数据")?
            .clone(),
    })?;
    Ok(result)
}

pub(super) fn execute_read_request(value: Value) -> Result<Value> {
    let request = super::super::json::exact(
        &value,
        &[
            "format_version",
            "kind",
            "scope_id",
            "configuration",
            "bucket",
            "operation",
            "key",
            "if_match",
            "version",
            "destination",
            "max_bytes",
        ],
        "原生对象读取请求",
    )?;
    if request.get("format_version").and_then(Value::as_u64) != Some(1)
        || request.get("kind").and_then(Value::as_str) != Some("ryframe-native-s3-read")
    {
        return Err("原生对象读取请求版本或类型无效".into());
    }
    let scope = required_text(request.get("scope_id"), "读取 scope")?;
    super::super::input::name(scope)?;
    let bucket = required_text(request.get("bucket"), "读取桶")?;
    if !super::super::input::BUCKETS.contains(&bucket) {
        return Err("原生对象读取只能访问已登记的业务对象桶".into());
    }
    let key = required_text(request.get("key"), "读取 key")?;
    validate_key(scope, key, true)?;
    let optional_text = |field: &str| -> Result<Option<String>> {
        match request.get(field) {
            Some(Value::Null) => Ok(None),
            value => Ok(Some(required_text(value, field)?.to_owned())),
        }
    };
    let if_match = optional_text("if_match")?;
    let version = optional_text("version")?;
    let operation = match required_text(request.get("operation"), "读取操作")? {
        "head-object"
            if request.get("destination") == Some(&Value::Null)
                && request.get("max_bytes").and_then(Value::as_u64) == Some(0) =>
        {
            Operation::Head {
                key: key.to_owned(),
                if_match,
                version,
            }
        }
        "get-object" => {
            let destination = required_text(request.get("destination"), "读取输出文件")?;
            let destination = crate::local_test_path::validate_local_test_path(
                std::path::Path::new(destination),
                &crate::workspace::root_dir(),
                crate::local_test_path::LocalTestPathKind::OutputFile,
            )
            .map_err(|error| format!("读取输出文件无效：{error}"))?;
            let max_bytes = request
                .get("max_bytes")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0)
                .ok_or("读取字节上限必须为正整数")?;
            Operation::Get {
                key: key.to_owned(),
                if_match,
                version,
                destination,
                max_bytes,
            }
        }
        _ => return Err("原生对象读取操作、输出或字节上限不匹配".into()),
    };
    super::execute(Request {
        configuration: Configuration::read(
            request.get("configuration").ok_or("读取请求缺少对象配置")?,
        )?,
        bucket: bucket.to_owned(),
        operation,
    })
}

pub(super) fn execute_inspect_request(value: Value) -> Result<Value> {
    let request = super::super::json::exact(
        &value,
        &[
            "format_version",
            "kind",
            "scope_id",
            "configuration",
            "bucket",
            "operation",
            "prefix",
            "continuation",
        ],
        "原生对象范围观察请求",
    )?;
    if request.get("format_version").and_then(Value::as_u64) != Some(1)
        || request.get("kind").and_then(Value::as_str) != Some("ryframe-native-s3-inspect")
    {
        return Err("原生对象范围观察请求版本或类型无效".into());
    }
    let scope = required_text(request.get("scope_id"), "观察 scope")?;
    super::super::input::name(scope)?;
    let bucket = required_text(request.get("bucket"), "观察桶")?;
    if !super::super::input::BUCKETS.contains(&bucket) {
        return Err("原生对象范围观察只能访问已登记的业务对象桶".into());
    }
    let null = |field: &str| request.get(field) == Some(&Value::Null);
    let operation = match required_text(request.get("operation"), "观察操作")? {
        "head-bucket" if null("prefix") && null("continuation") => Operation::HeadBucket,
        "get-bucket-versioning" if null("prefix") && null("continuation") => Operation::Versioning,
        "list-objects-v2" => {
            let prefix = required_text(request.get("prefix"), "对象列表前缀")?;
            if prefix != format!("{scope}/") {
                return Err("对象列表前缀必须精确等于已登记 scope".into());
            }
            let continuation = match request.get("continuation") {
                Some(Value::Null) => None,
                value => Some(required_text(value, "对象列表游标")?.to_owned()),
            };
            if continuation
                .as_ref()
                .is_some_and(|value| value.len() > 4096)
            {
                return Err("对象列表游标超过 4096 字节".into());
            }
            Operation::List {
                prefix: prefix.to_owned(),
                continuation,
            }
        }
        _ => return Err("原生对象范围观察操作与参数不匹配".into()),
    };
    super::execute(Request {
        configuration: Configuration::read(
            request.get("configuration").ok_or("观察请求缺少对象配置")?,
        )?,
        bucket: bucket.to_owned(),
        operation,
    })
}

fn required_text<'a>(value: Option<&'a Value>, label: &str) -> Result<&'a str> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
        .ok_or_else(|| format!("{label}缺失或无效").into())
}

fn validate_key(scope: &str, key: &str, allow_owner: bool) -> Result<()> {
    let prefix = format!("{scope}/");
    let logical = key.strip_prefix(&prefix).unwrap_or_default();
    if logical.is_empty()
        || (!allow_owner && logical == ".ryframe-owner")
        || logical.contains('\\')
        || logical
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || key.chars().any(char::is_control)
        || key.len() > 1024
    {
        return Err("对象 key 越过目标 scope、指向未授权 owner 或无法安全传输".into());
    }
    Ok(())
}
