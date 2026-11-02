//! 将显式工具文件绑定到 Cargo 标准环境；不修改进程全局环境或收据结构。
use std::{collections::BTreeMap, path::Path, process::Command};

use crate::Result;

pub(in crate::recovery) fn configured_target(root: &Path) -> Result<Option<String>> {
    crate::recovery::runtime::dataset_authority::reads::tool_settings(root)?;
    let settings =
        ryframe_config::ToolSettings::load(root).map_err(|error| error.message().to_owned())?;
    if settings.generator.target_dir.is_none() {
        return Ok(None);
    }
    let target = settings
        .generator_target_dir(root)
        .map_err(|error| error.message().to_owned())?;
    Ok(Some(
        target
            .to_str()
            .ok_or("工具文件 generator.target_dir 必须是 UTF-8 路径；检查 config/tools.toml 或 .local-tests/tools.toml")?
            .to_owned(),
    ))
}

pub(in crate::recovery) fn apply(
    root: &Path,
    environment: &mut BTreeMap<String, String>,
) -> Result<Option<String>> {
    let target = configured_target(root)?;
    if let Some(target) = &target {
        environment.retain(|name, _| {
            if cfg!(windows) {
                !name.eq_ignore_ascii_case("CARGO_TARGET_DIR")
            } else {
                name != "CARGO_TARGET_DIR"
            }
        });
        environment.insert("CARGO_TARGET_DIR".into(), target.clone());
    }
    Ok(target)
}

pub(in crate::recovery) fn configure_command(root: &Path, command: &mut Command) -> Result<()> {
    if let Some(target) = configured_target(root)? {
        command.env("CARGO_TARGET_DIR", &target);
        eprintln!(
            "Cargo 标准目标目录已绑定工具文件 generator.target_dir，环境值 SHA256={}",
            crate::devex::runtime_native::artifacts::sha(target.as_bytes())
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/bridge/cargo_environment.rs"]
mod tests;
