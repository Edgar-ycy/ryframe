use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    path::{Component, Path, PathBuf},
};

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use super::{digest, json as native_json, source};
use crate::{
    Result,
    build_source::with_source_commit,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process,
};

mod receipt;

pub(super) const TOOLS: [(&str, &str, &str); 3] = [
    ("reset", "bin-reset", "ryframe-reset"),
    ("migrate", "bin-migrate", "ryframe-migrate"),
    ("tenant-data", "bin-tenant-data", "ryframe-tenant-data"),
];

pub(crate) trait Runner {
    fn version(&self, root: &Path, executable: &str, arguments: &[&str]) -> Result<String>;
    fn build(
        &self,
        root: &Path,
        arguments: &[String],
        stdout: &Path,
        stderr: &Path,
        _commit: &str,
    ) -> Result<()>;
}

pub(crate) struct DefaultRunner;

impl Runner for DefaultRunner {
    fn version(&self, root: &Path, executable: &str, arguments: &[&str]) -> Result<String> {
        let output = process::command_output_quiet(root, executable, arguments)?;
        Ok(output.trim().to_owned())
    }

    fn build(
        &self,
        root: &Path,
        arguments: &[String],
        stdout: &Path,
        stderr: &Path,
        _commit: &str,
    ) -> Result<()> {
        let (executable, arguments) = arguments.split_first().ok_or("维护工具 Cargo 命令为空")?;
        if executable != "cargo" {
            return Err("维护工具构建命令必须使用 cargo".into());
        }
        process::run_owned_with_env_redirected(
            root,
            executable,
            arguments,
            &[("CARGO_NET_OFFLINE", "true")],
            stdout,
            stderr,
        )
    }
}

pub(crate) fn build(root: &Path, directory: &Path) -> Result<Value> {
    build_with(root, directory, &DefaultRunner)
}

pub(crate) fn build_summary(directory: &Path) -> Value {
    json!({
        "status": "maintenance_build_created",
        "receipt": directory.join("build.json"),
        "resources_modified": false,
        "restore_qualified": false,
    })
}

pub(crate) fn build_with(root: &Path, directory: &Path, runner: &dyn Runner) -> Result<Value> {
    validate_local_test_path(directory, root, LocalTestPathKind::NewDirectory)
        .map_err(|error| format!("维护构建目录无效：{error}"))?;
    let target = target_directory(root)?;
    fs::create_dir(directory)?;
    let mut stage = "source_binding";
    let mut source_binding = None;
    let mut stdout = None;
    let mut stderr = None;
    let result: Result<Value> = (|| {
        let inventory = source::capture(root)?;
        source_binding = Some(source::binding_value(&inventory.source));
        stage = "toolchain";
        let versions = toolchain(root, runner)?;
        let artifacts = with_source_commit(root, &inventory.source.snapshot.head, || {
            let mut artifacts = BTreeMap::new();
            for (role, _feature, name) in TOOLS {
                stage = match role {
                    "reset" => "build_reset",
                    "migrate" => "build_migrate",
                    _ => "build_tenant_data",
                };
                let arguments = command(role, &target)?;
                let output = directory.join(format!("{role}.cargo.jsonl"));
                let log = directory.join(format!("{role}.cargo.log"));
                stdout = Some(output.clone());
                stderr = Some(log.clone());
                runner.build(
                    root,
                    &arguments,
                    &output,
                    &log,
                    &inventory.source.snapshot.head,
                )?;
                let artifact = cargo_artifact(root, &target, name, &output)?;
                let saved = freeze_artifact(&artifact, directory, name)?;
                let output_digest = digest::file(&output)?;
                let log_digest = digest::file(&log)?;
                let saved_digest = digest::file(&saved)?;
                artifacts.insert(
                    role.to_owned(),
                    json!({
                        "executable": saved,
                        "cargo_executable": artifact,
                        "command": arguments,
                        "cargo_output": {
                            "file": output.file_name().unwrap().to_string_lossy(),
                            "bytes": output_digest.bytes,
                            "sha256": output_digest.sha256,
                        },
                        "cargo_log": {
                            "file": log.file_name().unwrap().to_string_lossy(),
                            "bytes": log_digest.bytes,
                            "sha256": log_digest.sha256,
                        },
                        "bytes": saved_digest.bytes,
                        "sha256": saved_digest.sha256,
                    }),
                );
                stdout = None;
                stderr = None;
            }
            Ok(artifacts)
        })?;
        stage = "verify_source_and_toolchain";
        let after = source::capture(root)?;
        if source::binding_value(&after.source) != source::binding_value(&inventory.source)
            || toolchain(root, runner)? != versions
        {
            return Err("维护工具构建期间源码或工具链变化".into());
        }
        let receipt = json!({
            "format_version": 1,
            "kind": "devex-clone-tool-build",
            "backend_root": path_text(root, "后端根目录")?,
            "source": source::binding_value(&inventory.source),
            "source_inventory": source::inventory_value(&inventory),
            "toolchain": versions,
            "target_directory": path_text(&target, "维护构建 target")?,
            "artifacts": artifacts,
            "restore_qualified": false,
            "resources_modified": false,
        });
        stage = "publish_receipt";
        native_json::write_new(&directory.join("build.json"), &receipt)?;
        Ok(receipt)
    })();
    match result {
        Ok(receipt) => Ok(receipt),
        Err(error) => {
            let safe_error = redact(&error.to_string());
            let failure = json!({
                "status": "failed",
                "error_type": error_type(error.as_ref()),
                "stage": stage,
                "source": source_binding,
                "resources_modified": false,
                "stdout": stdout.as_deref().map(read_diagnostic_optional).unwrap_or_default(),
                "stderr": stderr.as_deref().map(read_diagnostic_optional).unwrap_or_default(),
            });
            native_json::write_new(&directory.join("failed.json"), &failure).map_err(|write| {
                format!("维护工具构建失败（{stage}）：{safe_error}；同时无法写入失败收据：{write}")
            })?;
            Err(format!(
                "维护工具构建失败（{stage}）：{safe_error}；诊断已写入 {}",
                directory.join("failed.json").display()
            )
            .into())
        }
    }
}

pub(crate) fn verify_evidence(
    backend: &Path,
    filename: &Path,
    expected: Option<&Value>,
) -> Result<Value> {
    receipt::verify_evidence(backend, filename, expected)
}

pub(crate) fn verify(backend: &Path, filename: &Path) -> Result<Value> {
    verify_with(backend, filename, &DefaultRunner)
}

pub(crate) fn verify_summary(receipt: &Path) -> Value {
    json!({
        "status": "maintenance_build_verified",
        "receipt": receipt,
        "resources_modified": false,
        "restore_qualified": false,
    })
}

pub(crate) fn verify_with(backend: &Path, filename: &Path, runner: &dyn Runner) -> Result<Value> {
    validate_local_test_path(filename, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("维护构建收据无效：{error}"))?;
    let before = digest::file(filename)?;
    let initial = source::capture(backend)?;
    let receipt = receipt::verify_evidence(backend, filename, None)?;
    if receipt["source"] != source::binding_value(&initial.source) {
        return Err("维护工具未绑定当前完整开发源码和工具链".into());
    }
    if let Some(inventory) = receipt.get("source_inventory")
        && inventory != &source::inventory_value(&initial)
    {
        return Err("维护工具来源清单与当前完整开发源码不一致".into());
    }
    let versions = toolchain(backend, runner)?;
    if receipt["toolchain"] != versions {
        return Err("维护工具未绑定当前完整开发源码和工具链".into());
    }
    if digest::file(filename)? != before
        || source::binding_value(&source::capture(backend)?.source)
            != source::binding_value(&initial.source)
    {
        return Err("校验维护工具期间源码或收据变化".into());
    }
    Ok(receipt)
}

pub(super) fn command(role: &str, target: &Path) -> Result<Vec<String>> {
    let (_, feature, name) = TOOLS
        .iter()
        .find(|(candidate, _, _)| *candidate == role)
        .ok_or_else(|| format!("未知维护工具角色：{role}"))?;
    Ok(vec![
        "cargo".into(),
        "build".into(),
        "--locked".into(),
        "-p".into(),
        "ryframe".into(),
        "--no-default-features".into(),
        "--features".into(),
        (*feature).into(),
        "--bin".into(),
        (*name).into(),
        "--jobs".into(),
        "4".into(),
        "--target-dir".into(),
        path_text(target, "维护构建 target")?,
        "--message-format=json".into(),
    ])
}

fn recorded_cargo_path(backend: &Path, name: &str, content: &str) -> Result<PathBuf> {
    let manifest = fs::canonicalize(backend.join("crates/ryframe/Cargo.toml"))?;
    let mut found = Vec::new();
    for line in content.lines() {
        let event = native_json::parse_fragment(line, "Cargo 构建事件")?;
        let target = event.get("target").and_then(Value::as_object);
        let manifest_path = event
            .get("manifest_path")
            .and_then(Value::as_str)
            .map(Path::new);
        if event.get("reason").and_then(Value::as_str) == Some("compiler-artifact")
            && target
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                == Some(name)
            && target
                .and_then(|value| value.get("kind"))
                .and_then(Value::as_array)
                .is_some_and(|value| value == &[json!("bin")])
            && manifest_path
                .and_then(|path| fs::canonicalize(path).ok())
                .as_deref()
                == Some(manifest.as_path())
            && event.get("executable").and_then(Value::as_str).is_some()
        {
            let path = PathBuf::from(event["executable"].as_str().unwrap());
            if !path.is_absolute() {
                return Err("Cargo 产物事件必须使用实际绝对路径".into());
            }
            found.push(normalize_cache_path(&path)?);
        }
    }
    if found.len() != 1 {
        return Err("维护工具必须恰好绑定一个当前 Workspace 的 Cargo 产物事件".into());
    }
    Ok(found.remove(0))
}

// 缓存可以清理；只解析已核对的现有祖先，不把链接或观察失败当作缺失。
pub(super) fn normalize_cache_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("维护缓存路径必须是绝对路径且不得包含目录跳转".into());
    }
    let mut existing = None;
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if digest::linked(&metadata) {
                    return Err("维护缓存路径不能经过链接或重解析点".into());
                }
                if ancestor != path && !metadata.is_dir() {
                    return Err("维护缓存路径的现有祖先必须是目录".into());
                }
                existing.get_or_insert(ancestor);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let existing = existing.ok_or("维护缓存路径没有可核验的现有祖先")?;
    let mut normalized = portable_path(&fs::canonicalize(existing)?);
    let missing = path.strip_prefix(existing)?;
    if !missing.as_os_str().is_empty() {
        normalized.push(missing);
    }
    Ok(normalized)
}

fn cargo_artifact(root: &Path, target: &Path, name: &str, output: &Path) -> Result<PathBuf> {
    let artifact = recorded_cargo_path(root, name, &fs::read_to_string(output)?)?;
    if !fs::canonicalize(&artifact)?.starts_with(fs::canonicalize(target)?)
        || artifact.ancestors().any(|value| {
            fs::symlink_metadata(value).is_ok_and(|metadata| digest::linked(&metadata))
        })
    {
        return Err("Cargo 维护工具产物不属于本次固定 target".into());
    }
    Ok(artifact)
}

fn freeze_artifact(source: &Path, directory: &Path, name: &str) -> Result<PathBuf> {
    let digest = digest::file(source)?;
    let mut file_name = OsString::from(name);
    if let Some(extension) = source.extension() {
        file_name.push(".");
        file_name.push(extension);
    }
    let saved = directory.join(file_name);
    fs::copy(source, &saved)?;
    OpenOptions::new().write(true).open(&saved)?.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&saved, fs::Permissions::from_mode(0o700))?;
    }
    if digest::file(source)? != digest || digest::file(&saved)? != digest {
        return Err("复制维护工具时二进制发生变化".into());
    }
    Ok(saved)
}

pub(crate) fn toolchain(root: &Path, runner: &dyn Runner) -> Result<Value> {
    let rustc = runner.version(root, "rustc", &["-Vv"])?;
    let cargo = runner.version(root, "cargo", &["-V"])?;
    if rustc.is_empty() || cargo.is_empty() {
        return Err("维护工具链版本输出为空".into());
    }
    Ok(json!({"rustc": rustc, "cargo": cargo}))
}

pub(super) fn target_directory(backend: &Path) -> Result<PathBuf> {
    let settings =
        ryframe_config::ToolSettings::load(backend).map_err(|error| error.message().to_owned())?;
    let target = if settings.generator.target_dir.is_some() {
        settings
            .generator_target_dir(backend)
            .map_err(|error| error.message().to_owned())?
    } else {
        backend.join("target").join("ci").join("backend")
    };
    let target = target.components().collect::<PathBuf>();
    for current in target.ancestors() {
        match fs::symlink_metadata(current) {
            Ok(metadata) if digest::linked(&metadata) => {
                return Err("维护工具构建缓存不能经过链接".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(target)
}

pub(super) fn path_text(path: &Path, label: &str) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{label}必须为 UTF-8").into())
}

#[cfg(windows)]
fn portable_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else {
        value
            .strip_prefix(r"\\?\")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf())
    }
}

#[cfg(not(windows))]
fn portable_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

fn read_diagnostic(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(redact(&String::from_utf8_lossy(&bytes)))
}

fn read_diagnostic_optional(path: &Path) -> String {
    read_diagnostic(path).unwrap_or_default()
}

fn redact(value: &str) -> String {
    let secrets = std::env::vars_os()
        .filter(|(name, value)| {
            let name = name.to_string_lossy().to_ascii_uppercase();
            !value.is_empty()
                && [
                    "KEY",
                    "PASSWORD",
                    "PASSWD",
                    "SECRET",
                    "TOKEN",
                    "CREDENTIAL",
                    "MYSQL_PWD",
                ]
                .iter()
                .any(|marker| name.contains(marker))
        })
        .filter_map(|(_, value)| value.into_string().ok())
        .collect::<Vec<_>>();
    redact_with(value, secrets)
}

pub(crate) fn redact_with(value: &str, secrets: impl IntoIterator<Item = String>) -> String {
    let mut secrets = secrets.into_iter().collect::<Vec<_>>();
    secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
    let mut result = value.to_owned();
    for secret in secrets {
        let percent = utf8_percent_encode(&secret, NON_ALPHANUMERIC).to_string();
        let plus = percent.replace("%20", "+");
        let escaped = serde_json::to_string(&secret)
            .unwrap_or_default()
            .trim_matches('"')
            .to_owned();
        for value in [secret, percent, plus, escaped] {
            if !value.is_empty() {
                result = result.replace(&value, "[REDACTED]");
            }
        }
    }
    result
}

fn error_type(error: &(dyn std::error::Error + 'static)) -> String {
    std::any::type_name_of_val(error)
        .rsplit("::")
        .next()
        .unwrap_or("Error")
        .to_owned()
}
