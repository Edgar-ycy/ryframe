use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::{GeneratedAsset, ResourceError};

pub(super) fn rust_assets(assets: &mut [GeneratedAsset]) -> Result<(), ResourceError> {
    let rust_indexes = assets
        .iter()
        .enumerate()
        .filter(|(_, asset)| asset.path.ends_with(".rs"))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if rust_indexes.is_empty() {
        return Ok(());
    }
    let workspace = find_workspace_root().ok_or_else(|| {
        ResourceError::new(
            "无法定位 rustfmt 工作区配置",
            "从包含 Cargo.toml 与 rustfmt.toml 的后端仓库根目录运行资源生成命令",
        )
    })?;
    let temp_parent = workspace.join("target");
    fs::create_dir_all(&temp_parent).map_err(|error| {
        ResourceError::file(
            &temp_parent,
            format!("无法创建 rustfmt 临时目录：{error}"),
            "确认后端 target 目录可写且磁盘空间充足",
        )
    })?;
    let temp = tempfile::Builder::new()
        .prefix("resource-rustfmt-")
        .tempdir_in(&temp_parent)
        .map_err(|error| {
            ResourceError::file(
                &temp_parent,
                format!("无法创建 rustfmt 临时目录：{error}"),
                "确认后端 target 目录可写且磁盘空间充足",
            )
        })?;
    let mut staged = Vec::with_capacity(rust_indexes.len());
    for (sequence, asset_index) in rust_indexes.into_iter().enumerate() {
        let path = temp.path().join(format!("asset-{sequence:04}.rs"));
        fs::write(&path, &assets[asset_index].content).map_err(|error| {
            ResourceError::file(
                &path,
                format!("无法暂存待格式化生成文件：{error}"),
                "确认 target 目录可写后重试",
            )
            .with_resource(&assets[asset_index].resource)
        })?;
        staged.push((asset_index, path));
    }

    let output = Command::new("rustfmt")
        .arg("--edition")
        .arg("2024")
        .arg("--config-path")
        .arg(workspace.join("rustfmt.toml"))
        .arg("--config")
        .arg("skip_children=true")
        .args(staged.iter().map(|(_, path)| path))
        .current_dir(&workspace)
        .output()
        .map_err(|error| {
            ResourceError::new(
                format!("无法启动 rustfmt：{error}"),
                "安装当前 rust-toolchain.toml 声明的 rustfmt component，并从后端仓库根目录重试",
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut error = ResourceError::new(
            format!("rustfmt 拒绝生成的 Rust 资产：{}", stderr.trim()),
            "根据错误修复对应 renderer；生成器未返回也不会写入未格式化资产",
        );
        if let Some((asset_index, _)) = staged
            .iter()
            .find(|(_, path)| stderr.contains(path.file_name().unwrap().to_string_lossy().as_ref()))
        {
            error = error
                .with_resource(&assets[*asset_index].resource)
                .with_file(&assets[*asset_index].path);
        }
        return Err(error);
    }
    for (asset_index, path) in staged {
        assets[asset_index].content = fs::read_to_string(&path).map_err(|error| {
            ResourceError::file(
                &path,
                format!("无法读取 rustfmt 结果：{error}"),
                "检查磁盘状态后重新生成；正式资产尚未写入",
            )
            .with_resource(&assets[asset_index].resource)
        })?;
    }
    Ok(())
}

fn find_workspace_root() -> Option<PathBuf> {
    let current = std::env::current_dir().ok()?;
    current
        .ancestors()
        .find(|path| path.join("Cargo.toml").is_file() && path.join("rustfmt.toml").is_file())
        .map(Path::to_path_buf)
}
