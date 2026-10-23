use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::{Map, Value, json};

use super::super::super::{clone::native, inputs};
use crate::Result;

const DATA_HEADER: &str = "-- ryframe-reference-data-v1\n";

pub(super) fn request(operation: &str, args: &Value) -> Result<Value> {
    match operation {
        "reference-plan-hash" => {
            exact(args, &["value"])?;
            Ok(Value::String(native::digest::plan_hash(&args["value"])?))
        }
        "reference-identifier" => identifier(args, false),
        "reference-scope-identifier" => identifier(args, true),
        "reference-safe-file" => safe_file_request(args),
        "reference-dataset-timeout" => dataset_timeout(args),
        "reference-configuration-binding" => configuration_binding(args),
        "reference-plan-validate" => validate_plan(args),
        "reference-inventory-validate" => validate_inventory(args),
        "reference-artifacts-verify" => verify_artifacts(args),
        "reference-copy-metadata" => copy_metadata(args),
        "reference-sql-line" => sql_line(args),
        "reference-sql-normalize" => normalize_dump(args),
        "reference-sql-dump" => validate_dump(args),
        "reference-object-index" => object_index(args),
        _ => Err("参考证据操作无效".into()),
    }
}

fn exact<'a>(value: &'a Value, fields: &[&str]) -> Result<&'a Map<String, Value>> {
    native::json::exact(value, fields, "参考证据参数")
}

fn text<'a>(value: &'a Value, label: &str) -> Result<&'a str> {
    value
        .as_str()
        .ok_or_else(|| format!("{label}必须为字符串").into())
}

fn identifier(args: &Value, scope: bool) -> Result<Value> {
    exact(args, &["value"])?;
    let value = text(&args["value"], "资源名称")?;
    if scope {
        inputs::domain::scope_identifier(value)?;
    } else {
        inputs::domain::identifier(value)?;
    }
    Ok(Value::String(value.to_owned()))
}

fn safe_relative(relative: &str) -> Result<()> {
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || relative.chars().any(char::is_control)
        || relative
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err("备份文件路径越界或格式无效".into());
    }
    Ok(())
}

fn linked(path: &Path) -> Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(true);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn safe_file(root: &Path, relative: &str, exists: bool) -> Result<PathBuf> {
    safe_relative(relative)?;
    if !root.is_absolute() || !root.is_dir() || linked(root)? {
        return Err("备份根目录必须是无链接的绝对目录".into());
    }
    let candidate = root.join(relative);
    let mut current = candidate.as_path();
    while !current.exists() {
        current = current.parent().ok_or("备份文件缺少有效父目录")?;
    }
    if linked(current)? || !fs::canonicalize(current)?.starts_with(fs::canonicalize(root)?) {
        return Err("备份文件通过链接越界".into());
    }
    if exists && (!candidate.is_file() || linked(&candidate)?) {
        return Err("备份文件缺失".into());
    }
    Ok(candidate)
}

fn safe_file_request(args: &Value) -> Result<Value> {
    exact(args, &["root", "relative", "exists"])?;
    let root = Path::new(text(&args["root"], "备份根目录")?);
    let exists = args["exists"].as_bool().ok_or("exists必须为布尔值")?;
    let result = safe_file(root, text(&args["relative"], "备份相对路径")?, exists)?;
    Ok(Value::String(result.to_string_lossy().into_owned()))
}

fn dataset_timeout(args: &Value) -> Result<Value> {
    exact(args, &["plan"])?;
    let timeout = args["plan"]
        .get("dataset")
        .and_then(|value| value.get("timeout_seconds"))
        .and_then(Value::as_u64)
        .filter(|value| (1..=604_800).contains(value))
        .ok_or("数据准备必须显式设置 1 至 604800 秒的阶段时限，独立于恢复时限")?;
    Ok(Value::from(timeout))
}

fn digest_file(path: &Path) -> Result<Value> {
    let digest = native::digest::file(path)?;
    Ok(json!({"bytes":digest.bytes,"sha256":digest.sha256}))
}

fn configuration_value(backend: &Path, directory: &Path, app_env: &str) -> Result<Value> {
    if !matches!(app_env, "dev" | "test") {
        return Err("参考配置环境只允许dev或test".into());
    }
    let backend = fs::canonicalize(backend)?;
    let directory = fs::canonicalize(directory)?;
    let local = backend.join(".local-tests");
    if !directory.is_dir()
        || linked(&directory)?
        || directory != backend.join("config") && !directory.starts_with(local)
    {
        return Err("参考配置目录必须是项目config或后端受管.local-tests目录".into());
    }
    let environment = format!("app.{app_env}.toml");
    let mut files = Map::new();
    for (index, name) in ["app.toml", environment.as_str()].iter().enumerate() {
        let file = directory.join(name);
        if index == 0 && !file.exists() {
            continue;
        }
        if !file.is_file() || linked(&file)? {
            return Err("参考配置文件缺失或经过链接".into());
        }
        files.insert((*name).to_owned(), digest_file(&file)?);
    }
    let value = json!({"app_env":app_env,"directory":directory,"files":files});
    let sha = native::digest::plan_hash(&value)?;
    Ok(json!({"app_env":app_env,"directory":directory,"files":value["files"],"sha256":sha}))
}

fn configuration_binding(args: &Value) -> Result<Value> {
    exact(args, &["backend", "binding", "directory", "app_env"])?;
    let backend = Path::new(text(&args["backend"], "后端目录")?);
    let value = if !args["directory"].is_null() {
        configuration_value(
            backend,
            Path::new(text(&args["directory"], "配置目录")?),
            text(&args["app_env"], "APP_ENV")?,
        )?
    } else {
        let binding = exact(
            &args["binding"],
            &["app_env", "directory", "files", "sha256"],
        )?;
        let expected = configuration_value(
            backend,
            Path::new(text(&binding["directory"], "配置目录")?),
            text(&binding["app_env"], "APP_ENV")?,
        )?;
        if expected != args["binding"] {
            return Err("参考配置绑定的文件或摘要发生变化".into());
        }
        expected
    };
    Ok(value)
}

fn validate_plan(args: &Value) -> Result<Value> {
    exact(args, &["plan", "backend"])?;
    inputs::domain::validate_plan(
        &args["plan"],
        Path::new(text(&args["backend"], "后端目录")?),
    )?;
    Ok(Value::Bool(true))
}

fn validate_inventory(args: &Value) -> Result<Value> {
    exact(args, &["plan", "inventory"])?;
    inputs::inventory::validate(&args["plan"], &args["inventory"])?;
    Ok(Value::Bool(true))
}

fn verify_artifacts(args: &Value) -> Result<Value> {
    exact(args, &["root", "manifest"])?;
    inputs::inventory::verify_artifacts(
        Path::new(text(&args["root"], "备份根目录")?),
        &args["manifest"],
    )?;
    Ok(Value::Bool(true))
}

fn copy_metadata(args: &Value) -> Result<Value> {
    exact(args, &["metadata"])?;
    native::metadata::copy(&args["metadata"])
}

fn tables(args: &Value) -> Result<BTreeSet<String>> {
    let values = args.as_array().ok_or("SQL表清单必须为数组")?;
    let mut tables = BTreeSet::new();
    for value in values {
        let name = text(value, "SQL表名")?;
        inputs::domain::identifier(name)?;
        if !tables.insert(name.to_owned()) {
            return Err("SQL表清单不能重复".into());
        }
    }
    if tables.is_empty() {
        return Err("SQL表清单不能为空".into());
    }
    Ok(tables)
}

fn validate_sql_line(line: &str, tables: &BTreeSet<String>) -> Result<()> {
    if line.len() > 16 * 1024 * 1024 {
        return Err("单行导出超过受限检查大小".into());
    }
    let structure =
        Regex::new(r"^INSERT INTO `([a-z0-9_-]+)` \((?:`[a-z0-9_]+`(?:, )?)+\) VALUES \((.*)\);$")?;
    let captures = structure
        .captures(line)
        .filter(|value| tables.contains(&value[1]))
        .ok_or("备份 SQL 只能向清单中的表插入显式列和值")?;
    let mut quoted = false;
    let mut escaped = false;
    let mut plain = String::new();
    for character in captures[2].chars() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '\'' {
                quoted = false;
            }
        } else if character == '\'' {
            quoted = true;
        } else {
            plain.push(character);
        }
    }
    let literals = Regex::new(r"(?:NULL|_binary|0x[0-9A-Fa-f]*|[0-9eE+.,\s-])+")?;
    if quoted || !literals.replace_all(&plain, "").is_empty() {
        return Err("备份 SQL 包含表达式、多语句或无效字符串".into());
    }
    Ok(())
}

fn sql_line(args: &Value) -> Result<Value> {
    exact(args, &["line", "tables"])?;
    validate_sql_line(text(&args["line"], "SQL行")?, &tables(&args["tables"])?)?;
    Ok(Value::Bool(true))
}

fn dump_lines(path: &Path, tables: &BTreeSet<String>, normalized: bool) -> Result<Vec<String>> {
    let source = fs::read_to_string(path).map_err(|_| "SQL转储必须是UTF-8普通文件")?;
    let session_setting = Regex::new(r"^/\*!\d+ SET .* \*/;$")?;
    let mut lines = Vec::new();
    for (index, line) in source.lines().enumerate() {
        if normalized && index == 0 {
            if format!("{line}\n") != DATA_HEADER {
                return Err("SQL 产物不是已验证的参考数据转储".into());
            }
            continue;
        }
        if !normalized && (line.is_empty() || session_setting.is_match(line)) {
            continue;
        }
        validate_sql_line(line, tables)?;
        lines.push(line.to_owned());
    }
    if normalized && source.lines().next().is_none() {
        return Err("SQL 产物不是已验证的参考数据转储".into());
    }
    Ok(lines)
}

fn normalize_dump(args: &Value) -> Result<Value> {
    exact(args, &["raw", "output", "tables"])?;
    let raw = Path::new(text(&args["raw"], "原始SQL")?);
    let output = Path::new(text(&args["output"], "规范SQL")?);
    let lines = dump_lines(raw, &tables(&args["tables"])?, false)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    file.write_all(DATA_HEADER.as_bytes())?;
    for line in lines {
        writeln!(file, "{line}")?;
    }
    file.sync_all()?;
    Ok(Value::Bool(true))
}

fn validate_dump(args: &Value) -> Result<Value> {
    exact(args, &["path", "tables"])?;
    dump_lines(
        Path::new(text(&args["path"], "SQL产物")?),
        &tables(&args["tables"])?,
        true,
    )?;
    Ok(Value::Bool(true))
}

fn object_index(args: &Value) -> Result<Value> {
    exact(args, &["root", "objects", "artifacts"])?;
    let root = Path::new(text(&args["root"], "备份根目录")?);
    let objects = native::json::object(&args["objects"], "对象库存")?;
    let bucket = text(objects.get("bucket").ok_or("对象库存缺少桶")?, "对象桶")?;
    if !inputs::domain::BUCKETS.contains(&bucket) {
        return Err("对象库存桶无效".into());
    }
    let path = safe_file(root, &format!("objects/{bucket}/index.json"), true)?;
    let index = native::json::read(&path)?;
    let entries = index["entries"].as_array().ok_or("对象索引条目无效")?;
    let expected = objects["entries"].as_array().ok_or("对象库存条目无效")?;
    let expected = expected
        .iter()
        .map(|entry| Ok((text(&entry["key"], "对象键")?.to_owned(), entry)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    if entries.len() != expected.len() {
        return Err("对象产物索引没有完整对应备份清单".into());
    }
    let artifacts = args["artifacts"].as_array().ok_or("产物清单无效")?;
    let declared = artifacts
        .iter()
        .map(|artifact| {
            Ok((
                text(&artifact["relative_path"], "产物路径")?.to_owned(),
                artifact,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut keys = BTreeSet::new();
    for entry in entries {
        let key = text(&entry["key"], "对象键")?;
        let file = text(&entry["file"], "对象文件")?;
        let inventory = expected
            .get(key)
            .ok_or("对象产物索引没有完整对应备份清单")?;
        let artifact = declared.get(file).ok_or("对象产物索引引用未知文件")?;
        if !keys.insert(key)
            || artifact["resource"] != format!("objects:{bucket}")
            || entry["bytes"] != inventory["bytes"]
            || entry["sha256"] != inventory["sha256"]
        {
            return Err("对象产物索引与原对象或文件摘要不一致".into());
        }
        let digest = native::digest::file(&safe_file(root, file, true)?)?;
        if entry["bytes"] != digest.bytes || entry["sha256"] != digest.sha256 {
            return Err("对象数据文件被篡改".into());
        }
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_hash_and_timeout_are_strict() {
        assert_eq!(
            request("reference-identifier", &json!({"value":"table_1"})).unwrap(),
            "table_1"
        );
        assert!(request("reference-identifier", &json!({"value":"Table"})).is_err());
        assert!(request("reference-scope-identifier", &json!({"value":"scope-a"})).is_ok());
        assert!(
            request(
                "reference-dataset-timeout",
                &json!({"plan":{"dataset":{"timeout_seconds":604801}}})
            )
            .is_err()
        );
        assert_eq!(
            request("reference-plan-hash", &json!({"value":{"中":"文","a":1}})).unwrap(),
            native::digest::plan_hash(&json!({"中":"文","a":1})).unwrap()
        );
    }

    #[test]
    fn metadata_preserves_fields_and_rejects_ambiguous_values() {
        let valid = json!({
            "ContentType":"text/plain; charset=utf-8", "CacheControl":null,
            "ContentDisposition":null, "ContentEncoding":null, "ContentLanguage":null,
            "Expires":"2026-09-04T00:00:00Z", "WebsiteRedirectLocation":null,
            "Metadata":{"x-test":"value"}
        });
        let result = request("reference-copy-metadata", &json!({"metadata":valid})).unwrap();
        assert_eq!(result["Metadata"]["x-test"], "value");
        let mut invalid = result;
        invalid["ContentType"] = Value::String("text/*".into());
        assert!(request("reference-copy-metadata", &json!({"metadata":invalid})).is_err());
    }

    #[test]
    fn sql_validation_accepts_literals_and_rejects_expressions() {
        let tables = json!(["sys_post"]);
        let valid =
            "INSERT INTO `sys_post` (`id`, `name`, `raw`) VALUES (1,'引号\\\'文本',0x4142);";
        assert!(request("reference-sql-line", &json!({"line":valid,"tables":tables})).is_ok());
        assert!(
            request(
                "reference-sql-line",
                &json!({"line":"INSERT INTO `sys_post` (`id`) VALUES (SLEEP(1));","tables":["sys_post"]})
            )
            .is_err()
        );
    }

    #[test]
    fn sql_normalization_is_create_only_and_validated() {
        let directory = tempfile::tempdir().unwrap();
        let raw = directory.path().join("raw.sql");
        let output = directory.path().join("data.sql");
        fs::write(
            &raw,
            "/*!40101 SET @OLD=1 */;\nINSERT INTO `sys_post` (`id`) VALUES (1);\n",
        )
        .unwrap();
        let args = json!({"raw":raw,"output":output,"tables":["sys_post"]});
        assert_eq!(normalize_dump(&args).unwrap(), Value::Bool(true));
        assert_eq!(
            fs::read_to_string(&output).unwrap(),
            "-- ryframe-reference-data-v1\nINSERT INTO `sys_post` (`id`) VALUES (1);\n"
        );
        assert!(normalize_dump(&args).is_err());
        assert!(validate_dump(&json!({"path":output,"tables":["sys_post"]})).is_ok());
    }
}
