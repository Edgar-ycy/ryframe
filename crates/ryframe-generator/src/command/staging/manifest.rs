use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use super::snapshot::{SourceSnapshot, safe_path};

/// 内部相对依赖仍指向候选副本，外部相对依赖只在副本中展开，避免移动后解析到其他库。
pub(super) fn normalize_external_paths(
    source: &SourceSnapshot,
    candidate: &Path,
) -> Result<Vec<SourceSnapshot>, String> {
    let mut external = BTreeSet::new();
    for relative in source
        .input_paths()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
    {
        let target = safe_path(candidate, relative)?;
        if !target.is_file() {
            continue;
        }
        let text = fs::read_to_string(&target)
            .map_err(|error| format!("无法读取候选 Cargo 清单 {}：{error}", target.display()))?;
        let mut manifest: toml::Value =
            toml::from_str(&text).map_err(|error| format!("候选 Cargo 清单无效：{error}"))?;
        let origin = source.root.join(relative);
        if normalize_manifest(
            &mut manifest,
            origin.parent().ok_or("Cargo 清单没有父目录")?,
            &source.root,
            candidate,
            &mut external,
        )? {
            fs::write(
                &target,
                toml::to_string_pretty(&manifest).map_err(|error| error.to_string())?,
            )
            .map_err(|error| format!("无法写入候选 Cargo 清单：{error}"))?;
        }
    }
    external
        .into_iter()
        .map(|root| SourceSnapshot::capture(&external_workspace(&root)?))
        .collect()
}

fn normalize_manifest(
    manifest: &mut toml::Value,
    origin: &Path,
    workspace: &Path,
    candidate: &Path,
    external: &mut BTreeSet<PathBuf>,
) -> Result<bool, String> {
    let mut changed = false;
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(value) = manifest.get_mut(section) {
            changed |= normalize_dependencies(value, origin, workspace, candidate, external)?;
        }
        if let Some(targets) = manifest
            .get_mut("target")
            .and_then(toml::Value::as_table_mut)
        {
            for (_, target) in targets.iter_mut() {
                if let Some(value) = target.get_mut(section) {
                    changed |=
                        normalize_dependencies(value, origin, workspace, candidate, external)?;
                }
            }
        }
    }
    if let Some(value) = manifest
        .get_mut("workspace")
        .and_then(|value| value.get_mut("dependencies"))
    {
        changed |= normalize_dependencies(value, origin, workspace, candidate, external)?;
    }
    if let Some(sources) = manifest
        .get_mut("patch")
        .and_then(toml::Value::as_table_mut)
    {
        for (_, dependencies) in sources.iter_mut() {
            changed |=
                normalize_dependencies(dependencies, origin, workspace, candidate, external)?;
        }
    }
    Ok(changed)
}

fn normalize_dependencies(
    dependencies: &mut toml::Value,
    origin: &Path,
    workspace: &Path,
    candidate: &Path,
    external: &mut BTreeSet<PathBuf>,
) -> Result<bool, String> {
    let Some(dependencies) = dependencies.as_table_mut() else {
        return Err("Cargo 依赖声明必须是表；本批未安装".into());
    };
    let mut changed = false;
    for (_, dependency) in dependencies.iter_mut() {
        let Some(path) = dependency.get("path").and_then(toml::Value::as_str) else {
            continue;
        };
        let path = Path::new(path);
        let absolute = match fs::canonicalize(if path.is_absolute() {
            path.to_path_buf()
        } else {
            origin.join(path)
        }) {
            Ok(path) => path,
            // 未启用的可选依赖和 vendor 的开发依赖可以不在源码包中；实际解析仍由 Cargo 决定。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "无法解析本地 Cargo 依赖 {}：{error}；核对 path，本批未安装",
                    origin.join(path).display()
                ));
            }
        };
        if absolute.starts_with(workspace) && path.is_absolute() {
            let relative = absolute
                .strip_prefix(workspace)
                .map_err(|_| "内部依赖路径不属于源码工作区")?;
            dependency
                .as_table_mut()
                .ok_or("Cargo 路径依赖不是表")?
                .insert(
                    "path".into(),
                    toml::Value::String(cargo_path(&candidate.join(relative))),
                );
            changed = true;
        } else if !absolute.starts_with(workspace) {
            external.insert(absolute.clone());
            if !path.is_absolute() {
                dependency
                    .as_table_mut()
                    .ok_or("Cargo 路径依赖不是表")?
                    .insert("path".into(), toml::Value::String(cargo_path(&absolute)));
                changed = true;
            }
        }
    }
    Ok(changed)
}

fn cargo_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        value.strip_prefix(r"\\?\").unwrap_or(&value).to_owned()
    }
    #[cfg(not(windows))]
    {
        value.into_owned()
    }
}

fn external_workspace(root: &Path) -> Result<PathBuf, String> {
    for ancestor in root.ancestors() {
        let manifest = ancestor.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let text = fs::read_to_string(&manifest).map_err(|error| error.to_string())?;
        let value: toml::Value = toml::from_str(&text).map_err(|error| error.to_string())?;
        if ancestor == root
            && let Some(explicit) = value
                .get("package")
                .and_then(|package| package.get("workspace"))
                .and_then(toml::Value::as_str)
        {
            return fs::canonicalize(root.join(explicit))
                .map_err(|error| format!("外部依赖的 workspace 路径无效：{error}"));
        }
        if value.get("workspace").is_some() {
            return Ok(ancestor.to_path_buf());
        }
    }
    Ok(root.to_path_buf())
}
