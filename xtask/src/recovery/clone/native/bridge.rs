use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::{Value, json};

use super::{digest, json as native_json, maintenance, source};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const BRIDGE_BYTES: u64 = 16 * 1024 * 1024;
const ROLES: [&str; 2] = ["api", "worker"];

#[path = "bridge/build_context.rs"]
pub(crate) mod build_context;
#[path = "bridge/cargo_environment.rs"]
pub(in crate::recovery) mod cargo_environment;
#[path = "bridge/scope.rs"]
pub(crate) mod scope;

pub(crate) fn write(
    root: &Path,
    build_path: &Path,
    inventory_path: &Path,
    output_path: &Path,
) -> Result<Value> {
    validate_local_test_path(build_path, root, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("构建收据无效：{error}"))?;
    validate_local_test_path(inventory_path, root, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("来源清单无效：{error}"))?;
    validate_local_test_path(output_path, root, LocalTestPathKind::OutputFile)
        .map_err(|error| format!("来源桥接输出无效：{error}"))?;
    let build_binding = file_binding(build_path)?;
    let inventory_binding = file_binding(inventory_path)?;
    let receipt = read_binding(root, &build_binding)?;
    let inventory_value = read_binding(root, &inventory_binding)?;
    let inventory = source::validate_inventory(&inventory_value)?;
    let kind = receipt
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(kind, "restore-backend-build" | "devex-clone-tool-build") {
        return Err("来源桥接只接受 API/Worker 或维护 CLI 构建收据".into());
    }
    verify_build(root, build_path, &receipt)?;
    let original_fingerprints = verify_inventory_source(&inventory, &receipt)?;
    if kind == "restore-backend-build" && receipt["sources"]["full"] != inventory_value {
        return Err("API/Worker 构建收据与构建时 inventory 不一致".into());
    }
    let current_inventory = source::capture(root)?;
    let audited_source = source::execution_source(&current_inventory)?;
    let product_inputs = json!({
        "build": product_inputs(kind, &inventory, &original_fingerprints)?,
        "audited": product_inputs(kind, &current_inventory, &audited_source["fingerprints"])?,
    });
    if product_inputs["build"] != product_inputs["audited"] {
        return Err("产品构建输入已经变化，必须重新编译".into());
    }
    let bridge = json!({
        "format_version": 1,
        "kind": "devex-artifact-source-bridge",
        "backend_root": path_text(root, "后端根目录")?,
        "build": build_binding,
        "inventory": inventory_binding,
        "original_fingerprints": original_fingerprints,
        "audited_source": audited_source,
        "audited_inventory": source::inventory_value(&current_inventory),
        "product_inputs": product_inputs,
        "restore_qualified": false,
        "compiled": false,
    });
    if source::execution_source(&source::capture(root)?)? != audited_source {
        return Err("桥接核验期间源码发生变化".into());
    }
    native_json::write_new(output_path, &bridge)?;
    Ok(json!({
        "status": "artifact_reuse_audited",
        "receipt": path_text(output_path, "来源桥接输出")?,
        "compiled": false,
        "restore_qualified": false,
    }))
}

pub(crate) fn verify_build(root: &Path, path: &Path, receipt: &Value) -> Result<()> {
    match receipt.get("kind").and_then(Value::as_str) {
        Some("restore-backend-build") => verify_restore_receipt(receipt),
        Some("devex-clone-tool-build") => {
            let backend = maintenance_root(root, path, receipt)?;
            maintenance::verify_evidence(&backend, path, Some(receipt)).map(|_| ())
        }
        _ => Err("来源桥接只接受 API/Worker 或维护 CLI 构建收据".into()),
    }
}

fn verify_restore_receipt(receipt: &Value) -> Result<()> {
    let object = native_json::exact(
        receipt,
        &["format_version", "kind", "sources", "build", "artifacts"],
        "API/Worker 构建收据",
    )?;
    if object.get("format_version").and_then(Value::as_u64) != Some(2) {
        return Err("API/Worker 构建收据字段无效".into());
    }
    let sources = object.get("sources").ok_or("构建收据缺少来源分域")?;
    let full = sources.get("full").ok_or("构建收据来源分域缺少完整来源")?;
    let inventory = source::validate_inventory(full)?;
    source::validate_backend_domains(sources, &inventory)?;
    validate_build_context(object.get("build").ok_or("构建收据缺少构建上下文")?)?;
    verify_restore_artifacts(receipt)
}

pub(in crate::recovery) fn verify_restore_artifacts(receipt: &Value) -> Result<()> {
    let object = native_json::exact(
        receipt,
        &["format_version", "kind", "sources", "build", "artifacts"],
        "API/Worker构建收据",
    )?;
    if object["format_version"].as_u64() != Some(2) || object["kind"] != "restore-backend-build" {
        return Err("恢复二进制收据格式无效".into());
    }
    let artifacts = native_json::object(
        object.get("artifacts").ok_or("构建收据缺少产物")?,
        "API/Worker 构建产物",
    )?;
    let expected = ROLES
        .iter()
        .map(|role| (*role).to_owned())
        .collect::<BTreeSet<_>>();
    if artifacts.keys().cloned().collect::<BTreeSet<_>>() != expected {
        return Err("API/Worker 构建收据缺少完整产物".into());
    }
    let jobs = recorded_jobs(&receipt["build"]["jobs"])?;
    for role in ROLES {
        let artifact = artifacts
            .get(role)
            .ok_or("API/Worker 构建收据缺少完整产物")?;
        let artifact = native_json::exact(
            artifact,
            &["executable", "command", "bytes", "sha256"],
            "API/Worker 构建产物",
        )?;
        if artifact.get("command").and_then(Value::as_array)
            != Some(
                &build_command(role, jobs)?
                    .into_iter()
                    .map(Value::String)
                    .collect::<Vec<_>>(),
            )
        {
            return Err("API/Worker 构建产物字段无效".into());
        }
        let executable = PathBuf::from(native_json::text(
            artifact.get("executable").unwrap(),
            "API/Worker 产物路径",
        )?);
        if !executable.is_absolute() {
            return Err("API/Worker 产物路径必须是绝对路径".into());
        }
        let actual = digest::file(&executable)?;
        if actual.bytes
            != artifact["bytes"]
                .as_u64()
                .ok_or("API/Worker 产物字节数无效")?
            || actual.sha256
                != artifact["sha256"]
                    .as_str()
                    .ok_or("API/Worker 产物摘要无效")?
        {
            return Err("恢复二进制文件与构建收据不一致".into());
        }
    }
    Ok(())
}

pub(in crate::recovery) fn validate_build_context(value: &Value) -> Result<()> {
    let object = native_json::exact(
        value,
        &[
            "commands",
            "profile",
            "target",
            "jobs",
            "toolchain",
            "environment",
        ],
        "构建命令或工具链上下文",
    )?;
    let jobs = recorded_jobs(&value["jobs"])?;
    let commands = native_json::object(object.get("commands").unwrap(), "构建命令集合")?;
    if commands.len() != ROLES.len() {
        return Err("构建命令或工具链上下文无效".into());
    }
    for role in ROLES {
        if commands.get(role).and_then(Value::as_array)
            != Some(
                &build_command(role, jobs)?
                    .into_iter()
                    .map(Value::String)
                    .collect::<Vec<_>>(),
            )
        {
            return Err("构建命令或工具链上下文无效".into());
        }
    }
    if object.get("profile").and_then(Value::as_str) != Some("dev")
        || object
            .get("target")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || object
            .get("jobs")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("构建命令或工具链上下文无效".into());
    }
    let toolchain = native_json::exact(
        object.get("toolchain").unwrap(),
        &["cargo", "rustc"],
        "构建工具链上下文",
    )?;
    if toolchain
        .values()
        .any(|value| value.as_str().is_none_or(str::is_empty))
    {
        return Err("构建命令或工具链上下文无效".into());
    }
    validate_environment(object.get("environment").unwrap())
}

fn validate_environment(value: &Value) -> Result<()> {
    let environment = native_json::exact(value, &["variables", "sha256"], "构建环境摘要")?;
    let variables = native_json::array(environment.get("variables").unwrap(), "构建环境变量列表")?;
    let names = variables
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| "构建环境摘要无效".into())
        })
        .collect::<Result<BTreeSet<_>>>()?;
    if names.len() != variables.len()
        || variables
            .windows(2)
            .any(|pair| pair[0].as_str() > pair[1].as_str())
        || !Regex::new(r"^[a-f0-9]{64}$")?.is_match(
            environment
                .get("sha256")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
    {
        return Err("构建环境摘要无效".into());
    }
    Ok(())
}

fn verify_inventory_source(inventory: &source::Inventory, receipt: &Value) -> Result<Value> {
    let expected = build_source(receipt)?;
    if source::binding_value(&inventory.source) != expected
        || !Regex::new(r"^sha256:[a-f0-9]{64}$")?.is_match(&inventory.source.worktree_fingerprint)
    {
        return Err("原始文件清单没有绑定构建时的完整源码".into());
    }
    source::fingerprints(&inventory.files)
}

fn build_source(receipt: &Value) -> Result<Value> {
    match receipt.get("kind").and_then(Value::as_str) {
        Some("devex-clone-tool-build") => receipt
            .get("source")
            .cloned()
            .ok_or_else(|| "维护构建收据缺少来源".into()),
        Some("restore-backend-build") => receipt
            .get("sources")
            .and_then(|value| value.get("full"))
            .and_then(|value| value.get("source"))
            .cloned()
            .ok_or_else(|| "构建来源类型无效".into()),
        _ => Err("构建来源类型无效".into()),
    }
}

fn product_inputs(
    kind: &str,
    inventory: &source::Inventory,
    fingerprints: &Value,
) -> Result<Value> {
    match kind {
        "restore-backend-build" => Ok(source::backend_domains(inventory)?["product"].clone()),
        "devex-clone-tool-build" => Ok(fingerprints["product"].clone()),
        _ => Err("构建来源类型无效".into()),
    }
}

pub(in crate::recovery) fn build_jobs(root: &Path) -> Result<u8> {
    crate::recovery::runtime::dataset_authority::reads::tool_settings(root)?;
    Ok(ryframe_config::ToolSettings::load(root)
        .map_err(|error| error.message().to_owned())?
        .verification
        .build_jobs)
}

pub(in crate::recovery) fn recorded_jobs(value: &Value) -> Result<u8> {
    value
        .as_str()
        .and_then(|s| s.parse::<u8>().ok())
        .filter(|jobs| (1..=64).contains(jobs) && value.as_str() == Some(jobs.to_string().as_str()))
        .ok_or_else(|| {
            "构建来源 jobs 必须是 1 到 64 的规范整数文本；核对构建命令与原收据，不能改写历史证据"
                .into()
        })
}

pub(in crate::recovery) fn build_command(role: &str, jobs: u8) -> Result<Vec<String>> {
    if !(1..=64).contains(&jobs) {
        return Err("verification.build_jobs 必须是 1 到 64 的整数；核对工具配置".into());
    }
    let (feature, name) = match role {
        "api" => ("bin-api", "ryframe"),
        "worker" => ("bin-worker", "ryframe-worker"),
        _ => return Err("API/Worker 构建角色无效".into()),
    };
    Ok(vec![
        "cargo".into(),
        "build".into(),
        "--locked".into(),
        "-p".into(),
        "ryframe".into(),
        "--no-default-features".into(),
        "--features".into(),
        feature.into(),
        "--bin".into(),
        name.into(),
        "--jobs".into(),
        jobs.to_string(),
        "--message-format=json".into(),
    ])
}

fn maintenance_root(root: &Path, path: &Path, receipt: &Value) -> Result<PathBuf> {
    let current = crate::recovery::fixture_control::portable_path(&fs::canonicalize(root)?);
    let declared = PathBuf::from(
        receipt
            .get("backend_root")
            .and_then(Value::as_str)
            .ok_or("维护构建缺少可用的所属工作树")?,
    );
    if !declared.is_absolute() || !declared.is_dir() {
        return Err("维护构建缺少可用的所属工作树".into());
    }
    let declared = crate::recovery::fixture_control::portable_path(&fs::canonicalize(declared)?);
    if declared == current {
        return Ok(current);
    }
    let evidence = current.join(".local-tests");
    if declared == evidence || !declared.starts_with(&evidence) {
        return Err("隔离维护构建工作树必须位于当前 .local-tests".into());
    }
    let relative = declared
        .strip_prefix(&evidence)
        .map_err(|_| "隔离维护构建工作树必须位于当前 .local-tests")?;
    let mut cursor = evidence;
    for component in relative.components() {
        cursor.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&cursor)?;
        if digest::linked(&metadata) {
            return Err("隔离维护构建工作树不能经过链接".into());
        }
    }
    if !path.starts_with(declared.join(".local-tests")) {
        return Err("维护构建收据必须位于所属工作树的 .local-tests".into());
    }
    Ok(declared)
}

fn file_binding(path: &Path) -> Result<Value> {
    if !path.is_absolute() {
        return Err("来源审计只接受明确绝对路径".into());
    }
    Ok(json!({
        "path": path_text(path, "来源审计路径")?,
        "sha256": digest::file(path)?.sha256,
    }))
}

fn read_binding(root: &Path, binding: &Value) -> Result<Value> {
    let binding = native_json::exact(binding, &["path", "sha256"], "来源审计绑定")?;
    let path = PathBuf::from(native_json::text(
        binding.get("path").unwrap(),
        "来源审计路径",
    )?);
    if !path.is_absolute() {
        return Err("来源审计只接受明确绝对路径".into());
    }
    validate_local_test_path(&path, root, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("来源审计路径无效：{error}"))?;
    let metadata = fs::metadata(&path)?;
    if metadata.len() > BRIDGE_BYTES || file_binding(&path)? != Value::Object(binding.clone()) {
        return Err("来源审计文件摘要变化或超过限制".into());
    }
    let value = native_json::read(&path)?;
    native_json::object(&value, "来源审计文件")?;
    Ok(value)
}

fn path_text(path: &Path, label: &str) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{label}必须为 UTF-8").into())
}
