//! 来源配置与数据库凭据的唯一权威；秘密仅在内存比较、脱敏或写入显式私有配置。
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use super::documents;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::clone::native::{digest, target_verify::input::request_physical},
};

#[cfg(test)]
#[path = "binding/tests.rs"]
mod tests;

pub(super) fn physical(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "backend",
            "evidence_root",
            "source",
            "app_env",
            "config_dir",
        ],
        "来源绑定参数",
    )?;
    let backend = path(&args["backend"])?;
    let evidence = path(&args["evidence_root"])?;
    if args["app_env"] != "test" {
        return Err("来源环境必须明确为test".into());
    }
    let (directory, files) = configuration_files(&backend, &args["config_dir"])?;
    let table = crate::rate_limit::load_app_table(&backend, Some(&directory), "test")
        .map_err(|_| "来源配置不完整或无法安全解析；检查选定配置目录和app.test.toml")?;
    let table = serde_json::to_value(table).map_err(|_| "来源配置无法安全解析")?;
    let value = physical_table(&evidence, &args["source"], &table)?;
    if configuration_files(&backend, &args["config_dir"])? != (directory, files) {
        return Err("来源配置在读取期间发生变化".into());
    }
    Ok(value)
}

fn configuration_files(
    backend: &Path,
    declared: &Value,
) -> Result<(PathBuf, Vec<(PathBuf, digest::FileDigest)>)> {
    let declared = if declared.is_null() {
        backend.join("config")
    } else {
        let text = declared
            .as_str()
            .filter(|value| {
                !value.is_empty() && value.trim() == *value && !value.chars().any(char::is_control)
            })
            .ok_or("来源配置目录必须是明确文本路径")?;
        let directory = PathBuf::from(text);
        if directory
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err("来源配置目录不能包含父目录跳转".into());
        }
        if directory.is_absolute() {
            directory
        } else {
            backend.join(directory)
        }
    };
    for parent in declared.ancestors() {
        if digest::linked(
            &fs::symlink_metadata(parent).map_err(|_| "来源配置目录不存在或不可读取")?,
        ) {
            return Err("来源配置目录不能经过链接或重解析点".into());
        }
    }
    let directory = fs::canonicalize(declared).map_err(|_| "来源配置目录无法解析")?;
    #[cfg(windows)]
    let directory = PathBuf::from(
        directory
            .to_string_lossy()
            .strip_prefix(r"\\?\")
            .unwrap_or(&directory.to_string_lossy()),
    );
    validate_absolute_path(&directory, LocalTestPathKind::ExistingDirectory)?;
    let mut files = Vec::new();
    for name in ["app.toml", "app.test.toml"] {
        let file = directory.join(name);
        if name == "app.toml" && !file.exists() {
            continue;
        }
        validate_absolute_path(&file, LocalTestPathKind::ExistingFile)
            .map_err(|_| "来源所选配置文件缺失或经过链接")?;
        files.push((file.clone(), digest::file(&file)?));
    }
    Ok((directory, files))
}

fn physical_table(backend: &Path, source: &Value, table: &Value) -> Result<Value> {
    let databases = documents::array(&source["databases"], "来源数据库目标")?;
    let before = databases
        .iter()
        .map(|db| {
            let path = path(&db["defaults_file"])?;
            Ok((path.clone(), digest::file(&path)?))
        })
        .collect::<Result<Vec<_>>>()?;
    let selected = databases
        .iter()
        .map(|db| {
            let value = request_physical::defaults_connection(backend, db)?;
            Ok(json!({"key":db["key"],"host":value["host"],"port":value["port"]}))
        })
        .collect::<Result<Vec<_>>>()?;
    let value = request_physical::binding(
        backend,
        source,
        table,
        &BTreeMap::from([("APP_ENV".into(), "test".into())]),
        &json!({"databases":selected}),
    )?;
    for (path, expected) in before {
        if digest::file(&path)? != expected {
            return Err("来源数据库凭据在读取期间发生变化".into());
        }
    }
    Ok(value)
}

pub(super) fn path(value: &Value) -> Result<PathBuf> {
    let text = value
        .as_str()
        .filter(|value| {
            !value.is_empty() && value.trim() == *value && !value.chars().any(char::is_control)
        })
        .ok_or("来源配置缺少明确路径")?;
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err("来源配置必须使用绝对路径".into());
    }
    Ok(path)
}

pub(super) fn summary(args: &Value) -> Result<Value> {
    documents::exact(args, &["backend", "database"], "数据库公开连接参数")?;
    let mut value =
        request_physical::defaults_connection(&path(&args["backend"])?, &args["database"])?;
    value
        .as_object_mut()
        .ok_or("数据库连接无效")?
        .remove("password");
    Ok(value)
}

pub(super) fn redact(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["backend", "databases", "text"],
        "数据库诊断脱敏参数",
    )?;
    let root = path(&args["backend"])?;
    let mut text = args["text"]
        .as_str()
        .ok_or("数据库诊断必须为UTF-8文本")?
        .to_owned();
    let mut variants = std::collections::BTreeSet::new();
    let mut snapshots = Vec::new();
    for db in documents::array(&args["databases"], "诊断数据库绑定")? {
        let file = path(&db["defaults_file"])?;
        let before = digest::file(&file)?;
        let connection = request_physical::defaults_connection(&root, db)?;
        let password = connection["password"]
            .as_str()
            .ok_or("数据库凭据缺少密码")?;
        const ASCII: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
            .remove(b'-')
            .remove(b'_')
            .remove(b'.')
            .remove(b'~');
        let encoded = utf8_percent_encode(password, ASCII).to_string();
        let quoted = serde_json::to_string(password)?;
        let quoted = &quoted[1..quoted.len() - 1];
        let escaped = quoted
            .chars()
            .flat_map(|value| {
                if value.is_ascii() {
                    value.to_string().chars().collect::<Vec<_>>()
                } else {
                    value
                        .encode_utf16(&mut [0; 2])
                        .iter()
                        .map(|unit| format!("\\u{unit:04x}"))
                        .collect::<String>()
                        .chars()
                        .collect()
                }
            })
            .collect::<String>();
        variants.extend([
            password.to_owned(),
            encoded.clone(),
            encoded.replace("%20", "+"),
            quoted.into(),
            escaped,
        ]);
        snapshots.push((file, before));
    }
    let mut variants = variants.into_iter().collect::<Vec<_>>();
    variants.sort_by_key(|value| std::cmp::Reverse(value.len()));
    for value in variants {
        text = text.replace(&value, "[REDACTED]");
    }
    for (file, before) in snapshots {
        if digest::file(&file)? != before {
            return Err("数据库凭据在脱敏期间变化；不发布诊断".into());
        }
    }
    Ok(json!(text))
}

pub(super) fn configuration(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["backend", "database", "output", "text"],
        "夹具数据库配置写入参数",
    )?;
    let backend = path(&args["backend"])?;
    let output = path(&args["output"])?;
    validate_local_test_path(&output, &backend, LocalTestPathKind::OutputFile)?;
    if output.file_name().and_then(|value| value.to_str()) != Some("app.test.toml") {
        return Err("夹具数据库配置只能写入明确的app.test.toml".into());
    }
    let file = path(&args["database"]["defaults_file"])?;
    validate_absolute_path(&file, LocalTestPathKind::ExistingFile)?;
    let before = digest::file(&file)?;
    let connection = request_physical::defaults_connection(&backend, &args["database"])?;
    let mut table: toml::Table = args["text"]
        .as_str()
        .ok_or("夹具配置缺少TOML文本")?
        .parse()
        .map_err(|_| "夹具配置不是有效TOML；不回显配置内容")?;
    let primary = table
        .get_mut("database")
        .and_then(toml::Value::as_table_mut)
        .and_then(|value| value.get_mut("primary"))
        .and_then(toml::Value::as_table_mut)
        .ok_or("夹具配置缺少primary")?;
    fill_connection(primary, &connection)?;
    let targets = table
        .get_mut("tenant_data")
        .and_then(toml::Value::as_table_mut)
        .and_then(|value| value.get_mut("targets"))
        .and_then(toml::Value::as_array_mut)
        .ok_or("夹具缺少租户数据库目标")?;
    for target in targets {
        let target = target.as_table_mut().ok_or("夹具租户数据库目标无效")?;
        fill_connection(target, &connection)?;
    }
    if digest::file(&file)? != before {
        return Err("凭据在生成配置期间变化；不写入配置".into());
    }
    let encoded = toml::to_string(&table).map_err(|_| "夹具配置无法安全序列化")?;
    let mut stream = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    stream.write_all(encoded.as_bytes())?;
    stream.sync_all()?;
    let after = digest::file(&output)?;
    Ok(json!({"path":output,"bytes":after.bytes,"sha256":after.sha256}))
}

fn fill_connection(target: &mut toml::Table, connection: &Value) -> Result<()> {
    for field in ["host", "port", "tls_mode"] {
        let equal = match (target.get(field), &connection[field]) {
            (Some(toml::Value::String(actual)), Value::String(expected)) => actual == expected,
            (Some(toml::Value::Integer(actual)), Value::Number(expected)) => {
                Some(*actual) == expected.as_i64()
            }
            _ => false,
        };
        if !equal {
            return Err("夹具数据库目标与冻结MySQL连接不同".into());
        }
    }
    for field in ["username", "password"] {
        if target.contains_key(field) || target.contains_key(&format!("{field}_env")) {
            return Err("夹具输入不接受重复凭据或环境变量入口".into());
        }
        target.insert(
            field.into(),
            toml::Value::String(connection[field].as_str().ok_or("冻结连接缺少凭据")?.into()),
        );
    }
    Ok(())
}
