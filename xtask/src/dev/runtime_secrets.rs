use std::{collections::BTreeMap, env, fmt, fs, path::Path, process::Command};

use sha2::{Digest, Sha256};

use crate::Result;

const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "jwt_secret",
    "access_key",
    "secret_key",
    "metrics_bearer_token",
];

const ALLOWED_CONFIG_FILES: &[&str] = &[
    "app.toml",
    "app.dev.toml",
    "app.test.toml",
    "app.prod.toml",
    "feature-matrix.json",
];

#[derive(Clone, Copy)]
struct SecretSpec {
    environment: &'static str,
    path: &'static [&'static str],
}

const SECRET_SPECS: &[SecretSpec] = &[
    SecretSpec {
        environment: "APP_MONITOR_METRICS_BEARER_TOKEN",
        path: &["monitor", "metrics_bearer_token"],
    },
    SecretSpec {
        environment: "APP_DATABASE_PASSWORD",
        path: &["database", "primary", "password"],
    },
    SecretSpec {
        environment: "APP_AUTH_JWT_SECRET",
        path: &["auth", "jwt_secret"],
    },
    SecretSpec {
        environment: "APP_REDIS_PASSWORD",
        path: &["redis", "password"],
    },
    SecretSpec {
        environment: "APP_OBJECT_STORAGE_ACCESS_KEY",
        path: &["object_storage", "access_key"],
    },
    SecretSpec {
        environment: "APP_OBJECT_STORAGE_SECRET_KEY",
        path: &["object_storage", "secret_key"],
    },
];

/// 每代运行输入只在内存中持有的密钥环境覆盖；Debug 永不输出值。
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct RuntimeSecrets {
    values: BTreeMap<&'static str, String>,
}

/// 比较新的开发配置与 LKG 运行快照。比较前总是重新解析并校验源配置和密钥，
/// 只在持久化内容与仅内存的密钥投影都没有语义差异时返回 `true`。
pub(crate) fn runtime_config_matches_snapshot(
    source: &Path,
    snapshot: &Path,
    snapshot_secrets: &RuntimeSecrets,
) -> Result<bool> {
    let source_secrets = RuntimeSecrets::capture(source)?;
    if source_secrets != *snapshot_secrets {
        return Ok(false);
    }
    Ok(sanitized_config_fingerprint(source)? == sanitized_config_fingerprint(snapshot)?)
}

impl fmt::Debug for RuntimeSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeSecrets")
            .field("environment", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl RuntimeSecrets {
    pub(crate) fn capture(config_dir: &Path) -> Result<Self> {
        validate_config_tree(config_dir)?;
        let merged = load_development_config(config_dir)?;
        reject_unregistered_file_secrets(&merged, &mut Vec::new())?;
        let mut values = BTreeMap::new();
        for spec in SECRET_SPECS {
            if let Some(value) = resolve_secret(spec, &merged)?
                && !value.is_empty()
            {
                values.insert(spec.environment, value);
            }
        }
        Ok(Self { values })
    }

    pub(crate) fn apply(&self, command: &mut Command) {
        for spec in SECRET_SPECS {
            command.env_remove(format!("{}_FILE", spec.environment));
        }
        command.envs(self.values.iter().map(|(name, value)| (*name, value)));
    }

    pub(crate) fn environment_names(&self) -> Vec<String> {
        self.values.keys().map(ToString::to_string).collect()
    }

    pub(crate) fn require_environment(&self, required: &[String]) -> Result<()> {
        let missing = required
            .iter()
            .filter(|name| !self.values.contains_key(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "恢复 last-known-good 缺少当前进程密钥环境：{}",
                missing.join(", ")
            )
            .into())
        }
    }

    #[allow(dead_code)]
    pub(crate) fn registered_environment() -> Vec<&'static str> {
        SECRET_SPECS.iter().map(|spec| spec.environment).collect()
    }
}

pub(crate) fn snapshot_config_tree(source: &Path, target: &Path) -> Result<RuntimeSecrets> {
    let secrets = RuntimeSecrets::capture(source)?;
    copy_sanitized_tree(source, target)?;
    Ok(secrets)
}

fn resolve_secret(spec: &SecretSpec, merged: &toml::Table) -> Result<Option<String>> {
    let direct = read_environment(spec.environment)?;
    let file_environment = format!("{}_FILE", spec.environment);
    let file = read_environment(&file_environment)?;
    if direct.is_some() && file.is_some() {
        return Err(format!(
            "环境变量 {} 与 {file_environment} 不能同时设置",
            spec.environment
        )
        .into());
    }
    if let Some(value) = direct {
        return Ok(Some(value));
    }
    if let Some(path) = file {
        return read_secret_file(&file_environment, &path).map(Some);
    }
    Ok(value_at_path(merged, spec.path)
        .and_then(toml::Value::as_str)
        .map(ToOwned::to_owned))
}

fn read_environment(name: &str) -> Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(format!("环境变量 {name} 必须使用有效的 Unicode 编码").into())
        }
    }
}

fn read_secret_file(variable: &str, raw_path: &str) -> Result<String> {
    let path = raw_path.trim();
    if path.is_empty() {
        return Err(format!("环境变量 {variable} 不能是空路径").into());
    }
    let mut value = fs::read_to_string(path)
        .map_err(|error| format!("无法读取环境变量 {variable} 指向的密钥文件：{error}"))?;
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    if value.is_empty() {
        return Err(format!("环境变量 {variable} 指向的密钥文件不能为空").into());
    }
    Ok(value)
}

fn load_development_config(config_dir: &Path) -> Result<toml::Table> {
    let mut merged = toml::Table::new();
    for name in ["app.toml", "app.dev.toml"] {
        let path = config_dir.join(name);
        let input = match fs::read_to_string(&path) {
            Ok(input) => input,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let table = toml::from_str::<toml::Table>(&input)
            .map_err(|error| format!("无法解析运行配置 {}：{error}", path.display()))?;
        merge_tables(&mut merged, &table);
    }
    Ok(merged)
}

fn merge_tables(base: &mut toml::Table, overlay: &toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(base)), toml::Value::Table(overlay)) => {
                merge_tables(base, overlay);
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

fn value_at_path<'a>(table: &'a toml::Table, path: &[&str]) -> Option<&'a toml::Value> {
    let (first, rest) = path.split_first()?;
    let mut value = table.get(*first)?;
    for segment in rest {
        value = value.as_table()?.get(*segment)?;
    }
    Some(value)
}

fn reject_unregistered_file_secrets(value: &toml::Table, path: &mut Vec<String>) -> Result<()> {
    inspect_table(value, path)
}

fn inspect_table(table: &toml::Table, path: &mut Vec<String>) -> Result<()> {
    for (key, value) in table {
        path.push(key.clone());
        inspect_value(value, path)?;
        path.pop();
    }
    Ok(())
}

fn inspect_value(value: &toml::Value, path: &mut Vec<String>) -> Result<()> {
    match sensitive_value_action(value, path)? {
        SensitiveValueAction::NotSensitive => {}
        SensitiveValueAction::RegisteredString
        | SensitiveValueAction::TomlDatabasePassword
        | SensitiveValueAction::EmptyUnregisteredString => {
            return Ok(());
        }
    }
    match value {
        toml::Value::Table(table) => inspect_table(table, path),
        toml::Value::Array(items) => {
            path.push("[]".to_owned());
            for item in items {
                inspect_value(item, path)?;
            }
            path.pop();
            Ok(())
        }
        _ => Ok(()),
    }
}

#[derive(Clone, Copy)]
enum SensitiveValueAction {
    NotSensitive,
    RegisteredString,
    TomlDatabasePassword,
    EmptyUnregisteredString,
}

fn sensitive_value_action(value: &toml::Value, path: &[String]) -> Result<SensitiveValueAction> {
    if !SENSITIVE_KEYS.contains(&path.last().map(String::as_str).unwrap_or_default()) {
        return Ok(SensitiveValueAction::NotSensitive);
    }
    if is_toml_database_password(path) {
        return match value {
            toml::Value::String(_) => Ok(SensitiveValueAction::TomlDatabasePassword),
            _ => Err(format!(
                "运行配置包含数据库密码 {}，但值不是字符串；拒绝建立运行快照",
                path.join(".")
            )
            .into()),
        };
    }
    match (registered_path(path), value) {
        (true, toml::Value::String(_)) => Ok(SensitiveValueAction::RegisteredString),
        (false, toml::Value::String(secret)) if secret.is_empty() => {
            Ok(SensitiveValueAction::EmptyUnregisteredString)
        }
        (false, toml::Value::String(_)) => Err(format!(
            "运行配置包含未登记的敏感字段 {}；请改用受支持的 APP_* 密钥环境变量",
            path.join(".")
        )
        .into()),
        (registered, _) => {
            let ownership = if registered { "已登记" } else { "未登记" };
            Err(format!(
                "运行配置包含{ownership}的敏感字段 {}，但值不是字符串；拒绝建立运行快照",
                path.join(".")
            )
            .into())
        }
    }
}

fn registered_path(path: &[String]) -> bool {
    SECRET_SPECS.iter().any(|spec| {
        spec.path.len() == path.len()
            && spec
                .path
                .iter()
                .zip(path)
                .all(|(expected, actual)| *expected == actual)
    })
}

fn is_toml_database_password(path: &[String]) -> bool {
    matches!(path, [database, kind, index, password]
        if database == "database"
            && matches!(kind.as_str(), "replicas" | "sources")
            && index == "[]"
            && password == "password")
}

fn copy_sanitized_tree(source: &Path, target: &Path) -> Result<()> {
    validate_config_tree(source)?;
    copy_sanitized_directory(source, target)
}

fn sanitized_config_fingerprint(root: &Path) -> Result<String> {
    validate_config_tree(root)?;
    let mut entries = fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut digest = Sha256::new();
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let bytes = sanitized_config_file(&path)?;
        digest.update(name.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(bytes);
        digest.update([0]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn validate_config_tree(root: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("无法读取运行配置目录 {}：{error}", root.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(format!("运行配置快照要求普通目录：{}", root.display()).into());
    }
    let mut entries = fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        validate_config_entry(root, &entry.path(), entry.file_type()?)?;
    }
    Ok(())
}

fn validate_config_entry(root: &Path, path: &Path, file_type: fs::FileType) -> Result<()> {
    if file_type.is_symlink() {
        return Err(format!("运行配置快照拒绝符号链接：{}", path.display()).into());
    }
    if !file_type.is_file() {
        return Err(format!("运行配置快照拒绝目录或特殊文件：{}", path.display()).into());
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("运行配置文件越过快照根目录：{}", path.display()))?;
    if ALLOWED_CONFIG_FILES
        .iter()
        .any(|allowed| relative == Path::new(allowed))
    {
        Ok(())
    } else {
        Err(format!("运行配置快照拒绝未登记的配置文件：{}", path.display()).into())
    }
}

fn copy_sanitized_directory(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(format!("运行配置快照拒绝符号链接：{}", source_path.display()).into());
        }
        if !file_type.is_file() {
            return Err(
                format!("运行配置快照拒绝目录或特殊文件：{}", source_path.display()).into(),
            );
        }
        if is_toml(&source_path) {
            write_sanitized_toml(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn is_toml(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "toml")
}

fn write_sanitized_toml(source: &Path, target: &Path) -> Result<()> {
    fs::write(target, sanitized_config_file(source)?)?;
    Ok(())
}

fn sanitized_config_file(source: &Path) -> Result<Vec<u8>> {
    if !is_toml(source) {
        return Ok(fs::read(source)?);
    }
    let input = fs::read_to_string(source)?;
    let mut document = toml::from_str::<toml::Table>(&input)
        .map_err(|error| format!("无法解析运行配置 {}：{error}", source.display()))?;
    sanitize_table(&mut document, &mut Vec::new())?;
    Ok(toml::to_string_pretty(&document)?.into_bytes())
}

fn sanitize_table(table: &mut toml::Table, path: &mut Vec<String>) -> Result<()> {
    for (key, value) in table {
        path.push(key.clone());
        sanitize_value(value, path)?;
        path.pop();
    }
    Ok(())
}

fn sanitize_value(value: &mut toml::Value, path: &mut Vec<String>) -> Result<()> {
    match sensitive_value_action(value, path)? {
        SensitiveValueAction::RegisteredString => {
            let toml::Value::String(secret) = value else {
                unreachable!("敏感值动作已确认字符串类型");
            };
            secret.clear();
            return Ok(());
        }
        SensitiveValueAction::TomlDatabasePassword => return Ok(()),
        SensitiveValueAction::EmptyUnregisteredString => return Ok(()),
        SensitiveValueAction::NotSensitive => {}
    }
    match value {
        toml::Value::Table(table) => sanitize_table(table, path),
        toml::Value::Array(items) => {
            path.push("[]".to_owned());
            for item in items {
                sanitize_value(item, path)?;
            }
            path.pop();
            Ok(())
        }
        _ => Ok(()),
    }
}
