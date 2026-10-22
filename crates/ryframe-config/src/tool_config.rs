use ryframe_kernel::{AppError, AppResult};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// 开发工具配置从 TOML 文件读取，与服务运行配置分离。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolSettings {
    pub workspace: WorkspaceSettings,
    pub generator: GeneratorSettings,
    pub development: DevelopmentSettings,
    pub verification: VerificationSettings,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VerificationSettings {
    /// 检查工具的 Python 入口；相对路径以后端工作区为基准，不得包含父跳转或控制字符。
    pub python_program: Option<PathBuf>,
    /// 检查工具的 Node 入口；相对路径以后端工作区为基准，不得包含父跳转或控制字符。
    pub node_program: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DevelopmentSettings {
    pub config_dir: Option<PathBuf>,
    pub verify_tenant_targets: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceSettings {
    pub frontend_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneratorSettings {
    /// 应用配置目录；相对路径以后端工作区为基准，显式 CLI 或 Builder 参数优先。
    pub config_dir: Option<PathBuf>,
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
        values.try_into().map_err(|_: toml::de::Error| AppError::Config(
            "工具配置字段或类型错误：检查 config/tools.toml 和 .local-tests/tools.toml 的字段名与类型，路径以当前后端根目录解析".into()
        ))
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
    fn development_local_file_overrides_public_path_and_replaces_targets() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        fs::create_dir(root.path().join(".local-tests")).unwrap();
        fs::write(root.path().join("config/tools.toml"),"[development]\nconfig_dir='public-config'\nverify_tenant_targets=['shared-control','tenant-a']\n").unwrap();
        fs::write(
            root.path().join(".local-tests/tools.toml"),
            "[development]\nconfig_dir='本机 配置'\nverify_tenant_targets=['tenant-b']\n",
        )
        .unwrap();
        let settings = ToolSettings::load(root.path()).unwrap();
        assert_eq!(
            settings.development.config_dir,
            Some(PathBuf::from("本机 配置"))
        );
        assert_eq!(settings.development.verify_tenant_targets, ["tenant-b"]);
    }

    #[test]
    fn development_defaults_and_unknown_fields_remain_strict() {
        let settings: ToolSettings = toml::from_str("").unwrap();
        assert!(settings.development.config_dir.is_none());
        assert!(settings.development.verify_tenant_targets.is_empty());
        assert!(toml::from_str::<ToolSettings>("[development]\nconfig_directory='x'").is_err());
        assert!(
            toml::from_str::<ToolSettings>("[development]\nverify_tenant_targets='tenant-a'")
                .is_err()
        );
    }

    #[test]
    fn local_values_override_selected_fields_and_replace_arrays() {
        let mut base: toml::Value = toml::from_str("[workspace]\nfrontend_dir='../web'\n[generator]\nconfig_dir='public-config'\npackage='order-business'\ntables=['old']\nsync_frontend=true").unwrap();
        merge(
            &mut base,
            toml::from_str(
                "[generator]\nconfig_dir='本机 配置'\ntables=['a','b']\nsync_frontend=false",
            )
            .unwrap(),
        );
        let settings: ToolSettings = base.try_into().unwrap();
        assert_eq!(
            settings.generator.package.as_deref(),
            Some("order-business")
        );
        assert_eq!(settings.generator.tables, ["a", "b"]);
        assert!(!settings.generator.sync_frontend);
        assert_eq!(
            settings.generator.config_dir,
            Some(PathBuf::from("本机 配置"))
        );
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
        assert!(
            toml::from_str::<ToolSettings>("[generator]\nconfig_directory='wrong-key'").is_err()
        );
    }

    #[test]
    fn verification_python_uses_local_file_precedence_and_no_environment_override() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        fs::create_dir(root.path().join(".local-tests")).unwrap();
        fs::write(
            root.path().join("config/tools.toml"),
            "[verification]\npython_program='public/python.exe'\nnode_program='public/node.exe'\n",
        )
        .unwrap();
        assert_eq!(
            ToolSettings::load(root.path())
                .unwrap()
                .verification
                .python_program,
            Some("public/python.exe".into())
        );
        assert_eq!(
            ToolSettings::load(root.path())
                .unwrap()
                .verification
                .node_program,
            Some("public/node.exe".into())
        );
        fs::write(
            root.path().join(".local-tests/tools.toml"),
            "[verification]\npython_program='本机/python.exe'\nnode_program='本机/node.exe'\n",
        )
        .unwrap();
        assert_eq!(
            ToolSettings::load(root.path())
                .unwrap()
                .verification
                .python_program,
            Some("本机/python.exe".into())
        );
        assert_eq!(
            ToolSettings::load(root.path())
                .unwrap()
                .verification
                .node_program,
            Some("本机/node.exe".into())
        );
        assert!(
            ToolSettings::default()
                .verification
                .python_program
                .is_none()
        );
        let settings: ToolSettings = toml::from_str("").unwrap();
        assert!(settings.verification.python_program.is_none());
        assert!(settings.verification.node_program.is_none());
    }

    #[test]
    fn malformed_verification_values_and_unknown_keys_never_disclose_sentinel() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        for document in [
            "[verification]\npython_program={password='SECRET_SENTINEL'}\n",
            "[verification]\nnode_program={password='SECRET_SENTINEL'}\n",
            "[verification]\nSECRET_SENTINEL='unknown'\n",
            "[verification]\npython_program='SECRET_SENTINEL\n",
        ] {
            fs::write(root.path().join("config/tools.toml"), document).unwrap();
            let error = ToolSettings::load(root.path()).unwrap_err().to_string();
            assert!(!error.contains("SECRET_SENTINEL"));
            assert!(error.contains("工具配置"));
        }
    }
}
