use std::{fs, path::PathBuf};

pub(super) fn load() -> Result<(PathBuf, String, PathBuf), String> {
    let current = std::env::current_dir()
        .map_err(|error| format!("无法读取资源 Workspace 控制目录：{error}"))?;
    let path = current.join("resource-workspace-control.json");
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        format!(
            "资源 Workspace 缺少受管控制文件 {}：{error}",
            path.display()
        )
    })?;
    if !metadata.file_type().is_file() || metadata.len() > 16 * 1024 {
        return Err("资源 Workspace 控制文件必须是 16 KiB 内的普通文件".into());
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("无法读取资源 Workspace 控制文件：{error}"))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("资源 Workspace 控制文件不是有效 JSON：{error}"))?;
    let object = value
        .as_object()
        .ok_or("资源 Workspace 控制文件必须是对象")?;
    let expected = ["format_version", "frontend_dir", "profile", "target_dir"];
    if object.len() != expected.len() || expected.iter().any(|field| !object.contains_key(*field)) {
        return Err("资源 Workspace 控制文件字段无效".into());
    }
    if value["format_version"] != 1 {
        return Err("资源 Workspace 控制文件版本无效".into());
    }
    let path_field = |name: &str| -> Result<PathBuf, String> {
        let path = value[name]
            .as_str()
            .filter(|item| !item.trim().is_empty() && !item.contains(['\r', '\n', '\0']))
            .map(PathBuf::from)
            .ok_or_else(|| format!("资源 Workspace 控制字段 {name} 无效"))?;
        if !path.is_absolute() {
            return Err(format!("资源 Workspace 控制字段 {name} 必须是绝对路径"));
        }
        Ok(path)
    };
    let profile = value["profile"]
        .as_str()
        .filter(|value| matches!(*value, "full" | "targeted"))
        .ok_or("资源 Workspace 控制模式无效")?
        .to_owned();
    Ok((
        path_field("frontend_dir")?,
        profile,
        path_field("target_dir")?,
    ))
}
