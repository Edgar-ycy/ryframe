use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;

use crate::ResourceError;

#[derive(Clone, Debug)]
pub struct BusinessPackage {
    pub name: String,
    pub module: String,
    pub root: PathBuf,
    pub workspace_root: PathBuf,
}

#[derive(Deserialize)]
struct CargoMetadata {
    workspace_root: PathBuf,
    packages: Vec<CargoPackage>,
}

#[derive(Deserialize)]
struct CargoPackage {
    name: String,
    manifest_path: PathBuf,
    metadata: serde_json::Value,
}

pub fn locate_business_package(
    current_dir: &Path,
    package_name: &str,
) -> Result<BusinessPackage, ResourceError> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(current_dir)
        .output()
        .map_err(|error| command_error(format!("无法启动 cargo metadata：{error}")))?;
    if !output.status.success() {
        return Err(command_error(format!(
            "cargo metadata 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let metadata: CargoMetadata = serde_json::from_slice(&output.stdout).map_err(|error| {
        command_error(format!("cargo metadata 输出不是有效 JSON：{error}"))
    })?;
    let package = metadata
        .packages
        .into_iter()
        .find(|package| package.name == package_name)
        .ok_or_else(|| {
            command_error(format!("Workspace 中不存在 package `{package_name}`"))
        })?;
    let ryframe = package
        .metadata
        .get("ryframe")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| command_error("业务 crate 缺少 [package.metadata.ryframe]"))?;
    if ryframe.get("kind").and_then(serde_json::Value::as_str) != Some("business") {
        return Err(command_error("package.metadata.ryframe.kind 必须是 business"));
    }
    let module = ryframe
        .get("module")
        .and_then(serde_json::Value::as_str)
        .filter(|value| safe_key(value))
        .ok_or_else(|| command_error("package.metadata.ryframe.module 必须是安全的小写标识"))?;
    let root = package
        .manifest_path
        .parent()
        .ok_or_else(|| command_error("业务 crate Cargo.toml 没有父目录"))?
        .to_path_buf();
    Ok(BusinessPackage {
        name: package.name,
        module: module.to_owned(),
        root,
        workspace_root: metadata.workspace_root,
    })
}

fn safe_key(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

fn command_error(message: impl Into<String>) -> ResourceError {
    ResourceError::new(
        message,
        "确认命令位于 Cargo Workspace 内，且业务 crate 元数据完整",
    )
}
