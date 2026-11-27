use regex::Regex;
use serde_json::{Value, json};

use super::super::{input::BUCKETS, json as document, resources::common};
use crate::{Result, utc_timestamp::rfc3339_order_key};

pub(super) fn read_only(value: &Value, target: &Value) -> Result<()> {
    if value.get("transport").is_some() {
        return object(value, target);
    }
    if value.get("stdin").is_some() {
        return native_database_command(value);
    }
    let mut fields = vec!["command", "returncode", "error_type", "stdout", "stderr"];
    if value.get("stdout_file").is_some() {
        fields.push("stdout_file");
    }
    document::exact(value, &fields, "部门守卫命令收据")?;
    let command = document::array(&value["command"], "部门守卫命令")?;
    let command = command
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "部门守卫命令参数无效".into())
        })
        .collect::<Result<Vec<_>>>()?;
    if command.is_empty()
        || value["returncode"].as_i64() != Some(0)
        || !value["error_type"].is_null()
    {
        return Err("部门守卫命令不是成功的固定只读调用".into());
    }
    let executable = command[0]
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_lowercase();
    let accepted = match executable.as_str() {
        "mysql.exe" => match command.iter().position(|arg| *arg == "--execute") {
            Some(index) => command
                .get(index + 1)
                .is_some_and(|sql| sql.trim_start().to_uppercase().starts_with("SELECT ")),
            None => true,
        },
        "powershell.exe" => command.last().is_some_and(|script| {
            script.contains("Get-CimInstance Win32_Process")
                && script.ends_with(".CommandLine | ConvertTo-Json -Compress")
        }),
        "wsl.exe" => command.iter().any(|arg| {
            matches!(
                *arg,
                "/usr/bin/cat" | "/usr/bin/readlink" | "/usr/bin/sha256sum"
            )
        }),
        "cargo" => command[1..] == ["-V"],
        "rustc" => command[1..] == ["-Vv"],
        _ => false,
    };
    if !accepted {
        return Err("部门守卫包含未知或非只读外部命令".into());
    }
    Ok(())
}

fn native_database_command(value: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "command",
            "returncode",
            "error_type",
            "stdin",
            "stdout",
            "stderr",
            "stdout_file",
        ],
        "部门原生 MySQL 只读命令",
    )?;
    let [
        executable,
        defaults,
        database,
        batch,
        raw,
        no_columns,
        binary,
        charset,
        execute,
        sql,
    ] = document::array(&value["command"], "部门原生 MySQL 参数")?
    else {
        return Err("部门原生 MySQL 命令参数不完整".into());
    };
    let executable = document::text(executable, "部门原生 MySQL 程序")?
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    let check = match sql.as_str() {
        Some("SELECT @@server_uuid, DATABASE();") => "identity",
        Some(
            "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;",
        ) => "ownership",
        _ => return Err("部门原生 MySQL 只接受固定 UUID 或 owner 查询".into()),
    };
    let expected = Regex::new(&format!(r"^mysql-{check}-[a-f0-9]{{32}}\.stdout$"))?;
    let stdout = std::path::Path::new(document::text(
        &value["stdout_file"]["path"],
        "部门原生 MySQL stdout",
    )?);
    let valid_defaults = defaults
        .as_str()
        .and_then(|arg| arg.strip_prefix("--defaults-file="))
        .is_some_and(|path| std::path::Path::new(path).is_absolute());
    let valid_database = database
        .as_str()
        .and_then(|arg| arg.strip_prefix("--database="))
        .is_some_and(|name| {
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        });
    if !(executable.eq_ignore_ascii_case("mysql") || executable.eq_ignore_ascii_case("mysql.exe"))
        || !valid_defaults
        || !valid_database
        || ![batch, raw, no_columns, binary, charset, execute]
            .into_iter()
            .zip([
                "--batch",
                "--raw",
                "--skip-column-names",
                "--binary-mode",
                "--default-character-set=utf8mb4",
                "--execute",
            ])
            .all(|(actual, expected)| actual.as_str() == Some(expected))
        || !value["stdin"].is_null()
        || value["returncode"].as_i64() != Some(0)
        || !value["error_type"].is_null()
        || !value["stdout"].is_string()
        || value["stderr"] != ""
        || !stdout
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| expected.is_match(name))
    {
        return Err("部门原生 MySQL 命令不是完整成功的固定只读核验".into());
    }
    Ok(())
}

pub(super) fn query_status(value: &Value, kind: &str) -> Result<()> {
    document::exact(
        value,
        &[
            "format_version",
            "kind",
            "side",
            "target_key",
            "returncode",
            "checkpoint_passed",
            "remote_writes",
        ],
        "部门只读 SQL 安全收据",
    )?;
    let side = value["side"].as_str();
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != kind
        || !(side == Some("target") || kind == "administrator-row-read" && side == Some("source"))
        || value["target_key"] != "shared-control"
        || value["returncode"].as_i64() != Some(0)
        || value["checkpoint_passed"] != true
        || value["remote_writes"].as_u64() != Some(0)
    {
        return Err("部门只读 SQL 收据没有完整成功的检查点或固定目标".into());
    }
    Ok(())
}

pub(super) fn target_scope(target: &Value) -> Result<&str> {
    document::exact(
        target,
        &["scope_id", "endpoint", "configuration_sha256"],
        "部门对象目标投影",
    )?;
    let scope = document::text(&target["scope_id"], "部门对象 scope")?;
    common::scope_id(scope)?;
    if !Regex::new(r"^[a-f0-9]{64}$")?.is_match(document::text(
        &target["configuration_sha256"],
        "部门对象配置摘要",
    )?) {
        return Err("部门对象配置摘要无效".into());
    }
    Ok(scope)
}

fn object(value: &Value, target: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "transport",
            "operation",
            "bucket",
            "returncode",
            "error_type",
            "response",
            "key",
            "destination",
            "remote_writes",
            "scope_id",
            "endpoint",
            "configuration_sha256",
            "prefix",
            "continuation",
        ],
        "部门守卫对象收据",
    )?;
    let scope = target_scope(target)?;
    let bucket = document::text(&value["bucket"], "部门对象桶")?;
    let operation = document::text(&value["operation"], "部门对象操作")?;
    let response = &value["response"];
    let prefix = if operation == "list-objects-v2" {
        json!(format!("{scope}/"))
    } else {
        Value::Null
    };
    if value["transport"] != "rust-s3-sigv4"
        || !BUCKETS.contains(&bucket)
        || value["returncode"].as_i64() != Some(0)
        || !value["error_type"].is_null()
        || !response.is_object()
        || value["remote_writes"].as_u64() != Some(0)
        || !matches!(
            operation,
            "head-bucket" | "get-bucket-versioning" | "list-objects-v2" | "get-object"
        )
        || value["scope_id"] != target["scope_id"]
        || value["endpoint"] != target["endpoint"]
        || value["configuration_sha256"] != target["configuration_sha256"]
        || value["prefix"] != prefix
        || !value["continuation"].is_null()
    {
        return Err("部门守卫对象收据不是当前配置的成功只读调用".into());
    }
    if operation == "get-object" {
        return owner(value, scope, bucket);
    }
    if !value["key"].is_null() || !value["destination"].is_null() {
        return Err("部门守卫对象观察包含未知 key 或输出路径".into());
    }
    let accepted = match operation {
        "list-objects-v2" => {
            *response
                == json!({"IsTruncated":false,"Contents":[{"Key":format!("{scope}/.ryframe-owner")}]})
        }
        "get-bucket-versioning" => {
            *response == json!({}) || *response == json!({"MFADelete":"Disabled"})
        }
        _ => *response == json!({}),
    };
    if !accepted {
        return Err("部门守卫对象列表、端点或版本响应无效".into());
    }
    Ok(())
}

fn owner(value: &Value, scope: &str, bucket: &str) -> Result<()> {
    let destination = document::text(&value["destination"], "部门 owner 输出")?;
    if value["key"] != format!("{scope}/.ryframe-owner")
        || !Regex::new(&format!(
            r"^owner-target-{}-[a-f0-9]{{32}}\.txt$",
            regex::escape(bucket)
        ))?
        .is_match(destination)
    {
        return Err("部门守卫 owner 的作用域或输出路径无效".into());
    }
    owner_response(&value["response"], scope, bucket)
}

pub(super) fn owner_response(response: &Value, scope: &str, bucket: &str) -> Result<()> {
    let expected = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
    if response["ContentLength"].as_u64() != Some(expected.len() as u64)
        || !response["Metadata"].is_object()
        || ["ETag", "LastModified", "ContentType"]
            .iter()
            .any(|field| response[field].as_str().is_none_or(str::is_empty))
    {
        return Err("部门守卫 owner 响应元数据无效".into());
    }
    Ok(())
}

pub(super) fn native_owner_response(response: &Value, scope: &str, bucket: &str) -> Result<()> {
    owner_response(response, scope, bucket)?;
    for (name, value) in document::object(response, "部门原生 owner 响应")? {
        let valid = match name.as_str() {
            "ContentLength" | "MissingMeta" | "TagCount" | "PartsCount" => value.is_u64(),
            "LastModified" | "Expires" => value
                .as_str()
                .is_some_and(|value| rfc3339_order_key(value).is_ok()),
            "Metadata" => value
                .as_object()
                .is_some_and(|metadata| metadata.values().all(Value::is_string)),
            "ContentType"
            | "ETag"
            | "AcceptRanges"
            | "CacheControl"
            | "ContentDisposition"
            | "ContentEncoding"
            | "ContentLanguage"
            | "VersionId"
            | "StorageClass"
            | "WebsiteRedirectLocation"
            | "ServerSideEncryption"
            | "SSEKMSKeyId"
            | "ChecksumSHA256"
            | "ChecksumSHA1"
            | "ChecksumCRC32"
            | "ChecksumCRC32C"
            | "ChecksumCRC64NVME"
            | "ChecksumType"
            | "ReplicationStatus"
            | "Expiration"
            | "Restore"
            | "ObjectLockMode"
            | "ObjectLockRetainUntilDate"
            | "ObjectLockLegalHoldStatus" => value.is_string(),
            _ => false,
        };
        if !valid {
            return Err("部门原生 owner 响应包含未知字段或无效类型".into());
        }
    }
    Ok(())
}
