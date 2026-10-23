use crate::{Result, recovery::clone::native::s3::app_configuration};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 密码只在执行内存中保存，公开计划只有配置文件绑定及摘要。
pub(super) fn password(
    root: &Path,
    binding: &Value,
    snapshots: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<String> {
    let table = app_configuration::load(root, binding, snapshots)?;
    table
        .get("bootstrap")
        .and_then(toml::Value::as_table)
        .and_then(|value| value.get("admin_password"))
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            "绑定的app配置缺少bootstrap.admin_password；仅在该字段配置初始化管理员密码".into()
        })
}
