use std::{collections::BTreeSet, fs, path::Path, process::Command};

use super::{BusinessPackage, ResourceError, catalog_error};

pub(super) fn reader_manifest(package: &BusinessPackage) -> Result<String, ResourceError> {
    let lock = read_lock(&package.workspace_root.join("Cargo.lock"))?;
    let versions = packages(&lock)?
        .iter()
        .filter(|item| item.get("name").and_then(toml::Value::as_str) == Some("serde_json"))
        .filter_map(|item| item.get("version").and_then(toml::Value::as_str))
        .collect::<BTreeSet<_>>();
    if versions.len() != 1 {
        return Err(lock_error(
            "Workspace 锁文件中的 serde_json 版本不唯一或缺失",
        ));
    }
    let version = versions.first().expect("已检查唯一版本");
    let mut manifest: toml::Value = toml::from_str(&format!(
        "[package]\nname = 'ryframe-catalog-reader'\nversion = '0.0.0'\nedition = '2024'\n[workspace]\n[dependencies]\nbusiness = {{ package = {:?}, path = {:?}, default-features = false, features = ['catalog'] }}\nserde_json = '={version}'\n",
        package.name, package.root.to_string_lossy(),
    )).map_err(|error| lock_error(format!("catalog 临时清单无效：{error}")))?;
    let workspace = fs::read_to_string(package.workspace_root.join("Cargo.toml"))
        .map_err(|error| lock_error(format!("无法读取 Workspace 清单：{}", error.kind())))?;
    let workspace: toml::Value = toml::from_str(&workspace)
        .map_err(|error| lock_error(format!("Workspace TOML 无效：{error}")))?;
    if let Some(mut patches) = workspace.get("patch").cloned() {
        normalize_patch_paths(&mut patches, &package.workspace_root)?;
        manifest
            .as_table_mut()
            .expect("清单是表")
            .insert("patch".into(), patches);
    }
    toml::to_string(&manifest).map_err(|error| lock_error(format!("临时清单序列化失败：{error}")))
}

pub(super) fn resolve_reader(
    package: &BusinessPackage,
    scratch: &Path,
) -> Result<(), ResourceError> {
    let source = package.workspace_root.join("Cargo.lock");
    let root = read_lock(&source)?;
    fs::copy(&source, scratch.join("Cargo.lock"))
        .map_err(|error| lock_error(format!("无法复制 Workspace 锁文件：{}", error.kind())))?;
    // 只允许在临时目录登记 reader 包，沿用已有依赖版本；禁止网络及受控锁文件写入。
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--offline",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(scratch.join("Cargo.toml"))
        .current_dir(scratch)
        .output()
        .map_err(|error| lock_error(format!("无法启动 catalog 依赖解析：{error}")))?;
    if !output.status.success() {
        return Err(lock_error(format!(
            "catalog 离线解析失败（{}）：{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .with_file(source.to_string_lossy()));
    }
    validate_resolved(&root, &read_lock(&scratch.join("Cargo.lock"))?)
}

fn normalize_patch_paths(patches: &mut toml::Value, root: &Path) -> Result<(), ResourceError> {
    let sources = patches
        .as_table_mut()
        .ok_or_else(|| lock_error("patch 必须是表"))?;
    for (_, source) in sources.iter_mut() {
        let entries = source
            .as_table_mut()
            .ok_or_else(|| lock_error("patch 来源必须是表"))?;
        for (_, entry) in entries.iter_mut() {
            if let Some(path) = entry.get("path").and_then(toml::Value::as_str) {
                let path = Path::new(path);
                let absolute = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    root.join(path)
                };
                entry
                    .as_table_mut()
                    .ok_or_else(|| lock_error("patch 条目必须是表"))?
                    .insert(
                        "path".into(),
                        toml::Value::String(absolute.to_string_lossy().into_owned()),
                    );
            }
        }
    }
    Ok(())
}

fn read_lock(path: &Path) -> Result<toml::Value, ResourceError> {
    let text = fs::read_to_string(path).map_err(|error| {
        catalog_error(
            format!(
                "无法读取依赖锁文件（{}）；请先运行 cargo generate-lockfile --offline：{}",
                error.kind(),
                path.display()
            ),
            path,
        )
    })?;
    toml::from_str(&text)
        .map_err(|error| catalog_error(format!("依赖锁文件格式错误：{error}"), path))
}

fn packages(lock: &toml::Value) -> Result<&Vec<toml::Value>, ResourceError> {
    lock.get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| lock_error("锁文件缺少 package 数组"))
}

fn validate_resolved(root: &toml::Value, reader: &toml::Value) -> Result<(), ResourceError> {
    let identity = |item: &toml::Value| -> Result<_, ResourceError> {
        let field = |key| {
            item.get(key)
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        };
        Ok((
            field("name").ok_or_else(|| lock_error("锁文件缺少包名"))?,
            field("version").ok_or_else(|| lock_error("锁文件缺少版本"))?,
            field("source"),
            field("checksum"),
        ))
    };
    let expected = packages(root)?
        .iter()
        .map(identity)
        .collect::<Result<BTreeSet<_>, _>>()?;
    for item in packages(reader)? {
        let resolved = identity(item)?;
        if resolved.0 != "ryframe-catalog-reader" && !expected.contains(&resolved) {
            return Err(lock_error(format!(
                "catalog 依赖 {} {} 不在 Workspace 锁定来源中，未生成产品文件",
                resolved.0, resolved.1
            )));
        }
    }
    Ok(())
}

fn lock_error(message: impl Into<String>) -> ResourceError {
    ResourceError::new(
        message,
        "检查 Workspace Cargo.lock 和 patch；显式运行 cargo generate-lockfile --offline，缺少缓存时先安装项目锁定依赖",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_reader_can_only_add_itself_to_locked_dependencies() {
        let root: toml::Value = toml::from_str("[[package]]\nname='serde_json'\nversion='1.0.151'\nsource='registry+example'\nchecksum='original'").unwrap();
        let reader: toml::Value = toml::from_str(&format!(
            "{}\n[[package]]\nname='ryframe-catalog-reader'\nversion='0.0.0'",
            toml::to_string(&root).unwrap()
        ))
        .unwrap();
        validate_resolved(&root, &reader).unwrap();
        for (key, value) in [
            ("version", "1.0.152"),
            ("source", "registry+other"),
            ("checksum", "changed"),
        ] {
            let mut changed = reader.clone();
            changed["package"].as_array_mut().unwrap()[0][key] = toml::Value::String(value.into());
            assert!(
                validate_resolved(&root, &changed)
                    .unwrap_err()
                    .to_string()
                    .contains("锁定来源")
            );
        }
    }

    #[test]
    fn local_patch_paths_keep_the_workspace_origin() {
        let mut patches: toml::Value = toml::from_str("[crates-io]\nlocal = { path = 'vendor/local' }\nremote = { git = 'https://example.invalid/code', rev = 'abc' }").unwrap();
        let root = std::env::current_dir().unwrap();
        normalize_patch_paths(&mut patches, &root).unwrap();
        assert_eq!(
            patches["crates-io"]["local"]["path"].as_str().unwrap(),
            root.join("vendor/local").to_string_lossy()
        );
        assert_eq!(patches["crates-io"]["remote"]["rev"].as_str(), Some("abc"));
    }
}
