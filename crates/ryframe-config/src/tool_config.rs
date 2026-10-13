use ryframe_kernel::{AppError, AppResult};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// 开发工具配置独立于 APP_* 和服务运行配置，不包含连接密码。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolSettings {
    pub workspace: WorkspaceSettings,
    pub generator: GeneratorSettings,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceSettings {
    pub frontend_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneratorSettings {
    pub package: Option<String>,
    pub source: Option<String>,
    pub storage: Option<String>,
    pub tables: Vec<String>,
    pub models: Vec<String>,
    pub strip_table_prefix: Option<String>,
    pub sync_frontend: bool,
}

impl ToolSettings {
    pub fn load(root: &Path) -> AppResult<Self> {
        let mut values = toml::Value::Table(Default::default());
        for relative in ["config/tools.toml", ".local-tests/tools.toml"] {
            let path = root.join(relative);
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(AppError::Config(format!(
                        "无法读取工具配置 {}：{}；检查文件路径及读取权限",
                        path.display(),
                        error.kind()
                    )));
                }
            };
            let overlay = toml::from_str::<toml::Value>(&text).map_err(|error| {
                // 不输出原始配置行，避免错误文件意外含有凭据。
                AppError::Config(format!(
                    "工具配置 {} TOML 格式错误，位置 {:?}；检查表名、引号和字段类型",
                    path.display(),
                    error.span()
                ))
            })?;
            merge(&mut values, overlay);
        }
        values.try_into().map_err(|error: toml::de::Error| AppError::Config(format!(
            "工具配置字段错误：{}；检查 config/tools.toml 和 .local-tests/tools.toml，路径以当前后端根目录解析", error.message()
        )))
    }

    pub fn frontend_dir(&self, root: &Path) -> PathBuf {
        match &self.workspace.frontend_dir {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => root.join(path),
            None => root.parent().unwrap_or(root).join("ryframe-vue3"),
        }
    }
}

fn merge(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                if let Some(current) = base.get_mut(&key) {
                    merge(current, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_values_override_selected_fields_and_replace_arrays() {
        let mut base: toml::Value = toml::from_str("[workspace]\nfrontend_dir='../web'\n[generator]\npackage='order-business'\ntables=['old']\nsync_frontend=true").unwrap();
        merge(
            &mut base,
            toml::from_str("[generator]\ntables=['a','b']\nsync_frontend=false").unwrap(),
        );
        let settings: ToolSettings = base.try_into().unwrap();
        assert_eq!(
            settings.generator.package.as_deref(),
            Some("order-business")
        );
        assert_eq!(settings.generator.tables, ["a", "b"]);
        assert!(!settings.generator.sync_frontend);
        assert_eq!(
            settings.frontend_dir(Path::new("backend")),
            PathBuf::from("backend/../web")
        );
    }

    #[test]
    fn malformed_or_unknown_settings_are_rejected() {
        assert!(
            toml::from_str::<ToolSettings>("[workspace]\nfrontend_directory='../web'").is_err()
        );
        assert!(toml::from_str::<ToolSettings>("[generator]\nsync_frontend='yes'").is_err());
    }
}
