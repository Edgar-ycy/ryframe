use std::path::Path;
use std::{fs, process::Command};

use tempfile::Builder;

use super::{BusinessPackage, OwnedResourceDescriptor};
use crate::ResourceError;

mod lock;

pub(super) fn load_descriptors(
    package: &BusinessPackage,
    target: Option<&Path>,
) -> Result<Vec<OwnedResourceDescriptor>, ResourceError> {
    let manifest = lock::reader_manifest(package)?;
    let selected = catalog_target(package, target)?;
    let scratch_root = selected
        .parent()
        .ok_or_else(|| catalog_error("catalog 缓存缺少父目录", &selected))?
        .join("catalog");
    fs::create_dir_all(&scratch_root).map_err(|error| {
        catalog_error(format!("无法创建 catalog 临时目录：{error}"), &scratch_root)
    })?;
    let scratch = Builder::new()
        .prefix("catalog-")
        .tempdir_in(&scratch_root)
        .map_err(|error| {
            catalog_error(format!("无法创建 catalog 临时目录：{error}"), &scratch_root)
        })?;
    fs::write(scratch.path().join("Cargo.toml"), manifest).map_err(|error| {
        catalog_error(
            format!("无法写入 catalog 临时清单：{error}"),
            scratch.path(),
        )
    })?;
    fs::create_dir(scratch.path().join("src")).map_err(|error| {
        catalog_error(
            format!("无法创建 catalog 源码目录：{error}"),
            scratch.path(),
        )
    })?;
    fs::write(
        scratch.path().join("src/main.rs"),
        "fn main() { println!(\"{}\", serde_json::to_string(&business::resource_descriptors()).expect(\"资源描述符必须可序列化\")); }\n",
    )
    .map_err(|error| catalog_error(format!("无法写入 catalog reader：{error}"), scratch.path()))?;
    lock::resolve_reader(package, scratch.path(), &selected)?;
    let output = Command::new("cargo")
        .args(["run", "--locked", "--offline", "--quiet", "--manifest-path"])
        .arg(scratch.path().join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&selected)
        .output()
        .map_err(|error| catalog_error(format!("无法编译业务 catalog：{error}"), &package.root))?;
    if !output.status.success() {
        return Err(catalog_error(
            format!(
                "业务 catalog 编译失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            &package.root,
        ));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|error| {
        catalog_error(
            format!("业务 catalog 输出不是 UTF-8：{error}"),
            &package.root,
        )
    })?;
    let mut descriptors: Vec<OwnedResourceDescriptor> = serde_json::from_str(stdout.trim())
        .map_err(|error| {
            catalog_error(
                format!("resource_descriptors() 输出不是有效描述符 JSON：{error}"),
                &package.root,
            )
        })?;
    descriptors.sort_by(|left, right| left.name.cmp(&right.name));
    let mut names = std::collections::BTreeSet::new();
    let mut tables = std::collections::BTreeSet::new();
    for descriptor in &descriptors {
        if !names.insert(&descriptor.name) || !tables.insert(&descriptor.table) {
            return Err(catalog_error("业务资源名称或表名重复", &package.root));
        }
    }
    Ok(descriptors)
}

fn catalog_target(
    package: &BusinessPackage,
    selected: Option<&Path>,
) -> Result<std::path::PathBuf, ResourceError> {
    if let Some(target) = selected {
        return Ok(target.to_path_buf());
    }
    let settings = ryframe_config::ToolSettings::load(&package.workspace_root)
        .map_err(|error| catalog_error(error.message(), &package.root))?;
    crate::command::target_directory(&package.workspace_root, &settings)
        .map(|path| path.join("ryframe-generate/catalog-target"))
        .map_err(|error| catalog_error(error, &package.root))
}

fn catalog_error(message: impl Into<String>, path: &std::path::Path) -> ResourceError {
    ResourceError::new(
        message,
        "业务 crate 需提供 catalog feature 与 pub fn resource_descriptors() -> Vec<ResourceDescriptor>",
    )
    .with_file(path.to_string_lossy())
}
