use regex::Regex;
use serde_json::{Value, json};

use super::super::{input::BUCKETS, json as document, resources::common};
use crate::Result;

pub(super) fn read_only(value: &Value, target: &Value) -> Result<()> {
    if value.get("transport").is_some() {
        return object(value, target);
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
    let response = &value["response"];
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
