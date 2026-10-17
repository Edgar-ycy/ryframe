use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use toml::Value;

use super::policy::{LocalPatch, Policy, RegistrySource};

pub(crate) fn validate(root: &Path, policy: &Policy, cargo_home: Option<&Path>) -> Vec<String> {
    let (actual, mut errors) = local_path_patches(root);
    let configured = policy
        .local_patch_licenses
        .iter()
        .map(|patch| (patch.patch.to_lowercase(), patch))
        .collect::<BTreeMap<_, _>>();

    for (identity, (patch_name, path)) in &actual {
        match configured.get(identity) {
            None => errors.push(format!(
                "本地 patch 未登记许可证原件：{patch_name} ({path})"
            )),
            Some(entry) if entry.path != *path => errors.push(format!(
                "本地 patch 路径与策略不一致：{patch_name} Cargo={path}，策略={}",
                entry.path
            )),
            Some(_) => {}
        }
    }
    for (identity, entry) in &configured {
        if !actual.contains_key(identity) {
            errors.push(format!(
                "许可证策略包含不存在的本地 patch：{} ({})",
                entry.patch, entry.path
            ));
        }
    }

    let selected_cargo_home = cargo_home
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("CARGO_HOME").map(PathBuf::from))
        .or_else(default_cargo_home);
    for (identity, entry) in &configured {
        let Some((_, actual_path)) = actual.get(identity) else {
            continue;
        };
        if actual_path != &entry.path {
            continue;
        }
        validate_patch(root, entry, selected_cargo_home.as_deref(), &mut errors);
    }
    errors
}

fn local_path_patches(root: &Path) -> (BTreeMap<String, (String, String)>, Vec<String>) {
    let manifest_path = root.join("Cargo.toml");
    let manifest = match read_toml(&manifest_path, "Workspace Cargo.toml") {
        Ok(manifest) => manifest,
        Err(error) => return (BTreeMap::new(), vec![error]),
    };
    let Some(patch_table) = manifest.get("patch") else {
        return (BTreeMap::new(), Vec::new());
    };
    let Some(patch_table) = patch_table.as_table() else {
        return (
            BTreeMap::new(),
            vec!["Workspace Cargo.toml 的 [patch] 必须是表".to_owned()],
        );
    };
    let Some(crates_io) = patch_table.get("crates-io") else {
        return (BTreeMap::new(), Vec::new());
    };
    let Some(crates_io) = crates_io.as_table() else {
        return (
            BTreeMap::new(),
            vec!["Workspace Cargo.toml 的 [patch.crates-io] 必须是表".to_owned()],
        );
    };

    let mut patches = BTreeMap::new();
    let mut errors = Vec::new();
    for (patch_name, specification) in crates_io {
        let Some(specification) = specification.as_table() else {
            continue;
        };
        let Some(raw_path) = specification.get("path") else {
            continue;
        };
        let label = format!("[patch.crates-io].{patch_name}.path");
        let path = match raw_path.as_str() {
            Some(path) if safe_relative_path(path) => path.to_owned(),
            _ => {
                errors.push(format!("{label} 必须是安全的 POSIX 相对路径"));
                continue;
            }
        };
        let identity = patch_name.to_lowercase();
        if patches
            .insert(identity, (patch_name.clone(), path))
            .is_some()
        {
            errors.push(format!("Cargo 本地 patch 名称大小写冲突：{patch_name}"));
        }
    }
    (patches, errors)
}

fn validate_patch(
    root: &Path,
    entry: &LocalPatch,
    cargo_home: Option<&Path>,
    errors: &mut Vec<String>,
) {
    let crate_root = match resolve_regular_path(
        root,
        &entry.path,
        &format!("本地 patch {}", entry.patch),
        true,
    ) {
        Ok(path) => path,
        Err(error) => {
            errors.push(error);
            return;
        }
    };
    let manifest = match read_toml(
        &crate_root.join("Cargo.toml"),
        &format!("本地 patch {} Cargo.toml", entry.patch),
    ) {
        Ok(manifest) => manifest,
        Err(error) => {
            errors.push(error);
            return;
        }
    };
    let Some(package) = manifest.get("package").and_then(Value::as_table) else {
        errors.push(format!("本地 patch {} 缺少 [package]", entry.patch));
        return;
    };
    for (field, expected) in [
        ("name", entry.package.as_str()),
        ("version", entry.version.as_str()),
        ("license", entry.license_expression.as_str()),
    ] {
        let actual = package.get(field).and_then(Value::as_str);
        if actual != Some(expected) {
            errors.push(format!(
                "本地 patch {} 的 package.{field} 不一致：期望 {expected}，实际 {}",
                entry.patch,
                actual.unwrap_or("null")
            ));
        }
    }
    if package.contains_key("license-file") {
        errors.push(format!(
            "本地 patch {} 使用未建模的 package.license-file",
            entry.patch
        ));
    }

    for license_file in &entry.license_files {
        let label = format!("本地 patch {} {}", entry.patch, license_file.spdx);
        let path = match resolve_regular_path(
            &crate_root,
            &license_file.file,
            &format!("{label} 文件"),
            false,
        ) {
            Ok(path) => path,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let vendor_bytes = match fs::read(&path) {
            Ok(bytes) => canonical_license_bytes(&bytes),
            Err(error) => {
                errors.push(format!("{label} 文件无法读取：{}: {error}", path.display()));
                continue;
            }
        };
        let actual_hash = sha256_hex(&vendor_bytes);
        if actual_hash != license_file.sha256 {
            errors.push(format!(
                "{label} 登记哈希不一致：期望 {}，实际 {actual_hash}",
                license_file.sha256
            ));
        }
        if let Some(cargo_home) = cargo_home {
            validate_registry_copy(
                cargo_home,
                &license_file.registry_source,
                &vendor_bytes,
                &license_file.sha256,
                &label,
                errors,
            );
        }
    }
}

fn validate_registry_copy(
    cargo_home: &Path,
    source: &RegistrySource,
    vendor_bytes: &[u8],
    expected_hash: &str,
    label: &str,
    errors: &mut Vec<String>,
) {
    for package_root in registry_source_paths(cargo_home, source) {
        match source {
            RegistrySource::ManifestLicense {
                manifest_license, ..
            } => {
                let manifest = match read_toml(
                    &package_root.join("Cargo.toml"),
                    &format!("{label} registry Cargo.toml"),
                ) {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        errors.push(error);
                        continue;
                    }
                };
                let actual = manifest
                    .get("package")
                    .and_then(Value::as_table)
                    .and_then(|package| package.get("license"))
                    .and_then(Value::as_str);
                if actual != Some(manifest_license) {
                    errors.push(format!(
                        "{label} registry 清单许可证不一致：{} (期望 {manifest_license}，实际 {})",
                        package_root.join("Cargo.toml").display(),
                        actual.unwrap_or("null")
                    ));
                }
            }
            RegistrySource::File { file, .. } => {
                let source_path = match resolve_regular_path(
                    &package_root,
                    file,
                    &format!("{label} registry 原件"),
                    false,
                ) {
                    Ok(path) => path,
                    Err(error) => {
                        errors.push(error);
                        continue;
                    }
                };
                let source_bytes = match fs::read(&source_path) {
                    Ok(bytes) => canonical_license_bytes(&bytes),
                    Err(error) => {
                        errors.push(format!(
                            "{label} registry 原件无法读取：{}: {error}",
                            source_path.display()
                        ));
                        continue;
                    }
                };
                let source_hash = sha256_hex(&source_bytes);
                if source_hash != expected_hash || source_bytes != vendor_bytes {
                    errors.push(format!(
                        "{label} 与精确 registry 原件不一致：{} (期望 {expected_hash}，实际 {source_hash})",
                        source_path.display()
                    ));
                }
            }
        }
    }
}

fn registry_source_paths(cargo_home: &Path, source: &RegistrySource) -> Vec<PathBuf> {
    let registry_root = cargo_home.join("registry/src");
    if !registry_root.is_dir() {
        return Vec::new();
    }
    let (package, version) = match source {
        RegistrySource::File {
            package, version, ..
        }
        | RegistrySource::ManifestLicense {
            package, version, ..
        } => (package, version),
    };
    let package_dir = format!("{package}-{version}");
    let Ok(entries) = fs::read_dir(registry_root) else {
        return Vec::new();
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join(&package_dir).is_dir())
        .map(|path| path.join(&package_dir))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn read_toml(path: &Path, label: &str) -> Result<toml::Table, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("无法读取 {label} {}：{error}", path.display()))?;
    source
        .parse::<toml::Table>()
        .map_err(|error| format!("无法读取 {label} {}：{error}", path.display()))
}

fn resolve_regular_path(
    base: &Path,
    relative: &str,
    label: &str,
    directory: bool,
) -> Result<PathBuf, String> {
    let mut current = base.to_path_buf();
    for component in relative.split('/') {
        current.push(component);
        if current.exists() && link_like(&current) {
            return Err(format!(
                "{label} 不得经过符号链接或目录联接：{}",
                current.display()
            ));
        }
    }
    let base_resolved = fs::canonicalize(base)
        .map_err(|error| format!("{label} 不存在或无法读取：{} ({error})", current.display()))?;
    let resolved = fs::canonicalize(&current)
        .map_err(|error| format!("{label} 不存在或无法读取：{} ({error})", current.display()))?;
    if !resolved.starts_with(&base_resolved) {
        return Err(format!("{label} 逃逸出允许目录：{}", resolved.display()));
    }
    if directory && !resolved.is_dir() {
        return Err(format!("{label} 必须是目录：{}", resolved.display()));
    }
    if !directory && !resolved.is_file() {
        return Err(format!("{label} 必须是普通文件：{}", resolved.display()));
    }
    Ok(resolved)
}

#[cfg(windows)]
fn link_like(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn link_like(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('/')
            .all(|component| !component.is_empty() && !matches!(component, "." | ".."))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/'))
}

fn canonical_license_bytes(content: &[u8]) -> Vec<u8> {
    let mut normalized = Vec::with_capacity(content.len());
    let mut index = 0;
    while index < content.len() {
        if content[index] == b'\r' {
            normalized.push(b'\n');
            if content.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
        } else {
            normalized.push(content[index]);
        }
        index += 1;
    }
    normalized
}

fn sha256_hex(content: &[u8]) -> String {
    Sha256::digest(content)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn default_cargo_home() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .map(|home| home.join(".cargo"))
}
