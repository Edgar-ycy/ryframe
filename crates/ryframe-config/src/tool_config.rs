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
    pub linux_observer: Option<LinuxObserverSettings>,
    /// 检查工具的 Python 入口；相对路径以后端工作区为基准，不得包含父跳转或控制字符。
    pub python_program: Option<PathBuf>,
    /// 检查工具的 Node 入口；相对路径以后端工作区为基准，不得包含父跳转或控制字符。
    pub node_program: Option<PathBuf>,
    /// pnpm 元数据缓存目录；缺省保留 pnpm 标准行为，不创建目录。
    pub pnpm_cache_dir: Option<PathBuf>,
}

/// Linux 夹具工具的显式双端路径与冻结来源；观察与写入口分别授权，不读取环境覆盖。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LinuxObserverSettings {
    pub distribution: String,
    pub user: String,
    pub uid: u32,
    pub program: PathBuf,
    pub linux_program: String,
    pub source_root: PathBuf,
    pub linux_source_root: String,
    pub source_commit: String,
    pub sha256: String,
    pub source_sha256: String,
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
    /// CI 配套前端仓库；省略时由 GitHub 标准 owner 身份选择 ryframe-vue3。
    pub frontend_repository: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneratorSettings {
    /// 生成器 Cargo 缓存根目录；相对后端根目录解析，不读取环境覆盖。
    pub target_dir: Option<PathBuf>,
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

    pub fn generator_target_dir(&self, root: &Path) -> AppResult<PathBuf> {
        let selected = self
            .generator
            .target_dir
            .as_deref()
            .unwrap_or(Path::new("target"));
        resolve_tool_path(root, selected, "generator.target_dir")
    }

    pub fn pnpm_cache_dir(&self, root: &Path) -> AppResult<Option<PathBuf>> {
        self.verification
            .pnpm_cache_dir
            .as_deref()
            .map(|path| resolve_tool_path(root, path, "verification.pnpm_cache_dir"))
            .transpose()
    }

    pub fn frontend_dir(&self, root: &Path) -> PathBuf {
        match &self.workspace.frontend_dir {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => root.join(path),
            None => root.parent().unwrap_or(root).join("ryframe-vue3"),
        }
    }
}

fn resolve_tool_path(root: &Path, selected: &Path, field: &str) -> AppResult<PathBuf> {
    if !selected.is_absolute()
        && selected.components().any(|component| {
            matches!(
                component,
                std::path::Component::Prefix(_) | std::path::Component::RootDir
            )
        })
    {
        return Err(AppError::Config(format!(
            "{field} 不接受驱动器相对路径或缺少驱动器的根路径；使用完整绝对路径或以后端根目录为基准的普通相对路径"
        )));
    }
    if selected
        .to_str()
        .is_none_or(|value| value.trim().is_empty() || value.chars().any(char::is_control))
        || selected
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(AppError::Config(format!(
            "{field} 必须是非空 UTF-8 路径，不得包含控制字符或父跳转；核对工具配置"
        )));
    }
    Ok(if selected.is_absolute() {
        selected.to_owned()
    } else {
        root.join(selected)
    })
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
    fn linux_observer_is_optional_and_rejects_partial_unknown_or_mistyped_bindings() {
        assert!(
            ToolSettings::default()
                .verification
                .linux_observer
                .is_none()
        );
        let value = r#"[verification.linux_observer]
distribution='Ubuntu-24.04'
user='fixture'
uid=1000
program='D:/tool/xtask'
linux_program='/mnt/d/tool/xtask'
source_root='D:/source'
linux_source_root='/mnt/d/source'
source_commit='0123456789012345678901234567890123456789'
sha256='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
source_sha256='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
"#;
        let settings: ToolSettings = toml::from_str(value).unwrap();
        assert_eq!(settings.verification.linux_observer.unwrap().uid, 1000);
        for invalid in [
            value.replace("uid=1000", "uid=true"),
            value.replace("uid=1000", "uid=-1"),
            value.replace("uid=1000", ""),
            format!("{value}password_env='APP_PASSWORD'\n"),
        ] {
            assert!(toml::from_str::<ToolSettings>(&invalid).is_err());
        }
    }

    #[test]
    fn development_local_file_overrides_public_path_and_replaces_targets() {
        let root = crate::test_directory("tool-config-");
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
        assert!(settings.workspace.frontend_repository.is_none());
        assert!(settings.development.verify_tenant_targets.is_empty());
        assert!(toml::from_str::<ToolSettings>("[development]\nconfig_directory='x'").is_err());
        assert!(
            toml::from_str::<ToolSettings>("[development]\nverify_tenant_targets='tenant-a'")
                .is_err()
        );
    }

    #[test]
    fn local_values_override_selected_fields_and_replace_arrays() {
        let mut base: toml::Value = toml::from_str("[workspace]\nfrontend_dir='../web'\nfrontend_repository='public/frontend'\n[generator]\nconfig_dir='public-config'\npackage='order-business'\ntables=['old']\nsync_frontend=true").unwrap();
        merge(
            &mut base,
            toml::from_str(
                "[workspace]\nfrontend_repository='local/frontend'\n[generator]\nconfig_dir='本机 配置'\ntables=['a','b']\nsync_frontend=false",
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
            settings.workspace.frontend_repository.as_deref(),
            Some("local/frontend")
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
        assert!(toml::from_str::<ToolSettings>("[workspace]\nfrontend_repository=42").is_err());
        assert!(toml::from_str::<ToolSettings>("[generator]\nsync_frontend='yes'").is_err());
        assert!(
            toml::from_str::<ToolSettings>("[generator]\nconfig_directory='wrong-key'").is_err()
        );
    }

    #[test]
    fn verification_files_apply_local_precedence_and_default_to_standard_tools() {
        let root = crate::test_directory("tool-config-");
        fs::create_dir(root.path().join("config")).unwrap();
        fs::create_dir(root.path().join(".local-tests")).unwrap();
        fs::write(
            root.path().join("config/tools.toml"),
            "[verification]\npython_program='public/python.exe'\nnode_program='public/node.exe'\npnpm_cache_dir='public/cache'\n",
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
        assert_eq!(
            ToolSettings::load(root.path())
                .unwrap()
                .pnpm_cache_dir(root.path())
                .unwrap(),
            Some(root.path().join("public/cache"))
        );
        fs::write(
            root.path().join(".local-tests/tools.toml"),
            "[verification]\npython_program='本机/python.exe'\nnode_program='本机/node.exe'\npnpm_cache_dir='本机/cache with spaces'\n",
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
        assert!(settings.pnpm_cache_dir(root.path()).unwrap().is_none());
        assert_eq!(
            ToolSettings::load(root.path())
                .unwrap()
                .pnpm_cache_dir(root.path())
                .unwrap(),
            Some(root.path().join("本机/cache with spaces"))
        );
        assert!(!root.path().join("本机").exists());
    }

    #[test]
    fn pnpm_cache_paths_use_target_safety_rules_without_creating_directories() {
        let root = crate::test_directory("tool-cache-");
        let absolute = fs::canonicalize(root.path()).unwrap().join("未创建 缓存");
        let mut settings = ToolSettings::default();
        settings.verification.pnpm_cache_dir = Some(absolute.clone());
        assert_eq!(
            settings.pnpm_cache_dir(root.path()).unwrap(),
            Some(absolute.clone())
        );
        for invalid in ["", "  ", "cache/../outside", "cache\nSECRET_SENTINEL"] {
            settings.verification.pnpm_cache_dir = Some(invalid.into());
            let error = settings
                .pnpm_cache_dir(root.path())
                .unwrap_err()
                .to_string();
            assert!(error.contains("verification.pnpm_cache_dir"));
            assert!(!error.contains("SECRET_SENTINEL"));
        }
        #[cfg(windows)]
        for invalid in ["D:cache", "\\cache"] {
            settings.verification.pnpm_cache_dir = Some(invalid.into());
            assert!(settings.pnpm_cache_dir(root.path()).is_err());
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn malformed_verification_values_and_unknown_keys_never_disclose_sentinel() {
        let root = crate::test_directory("tool-config-");
        fs::create_dir(root.path().join("config")).unwrap();
        for document in [
            "[verification]\npython_program={password='SECRET_SENTINEL'}\n",
            "[verification]\nnode_program={password='SECRET_SENTINEL'}\n",
            "[verification]\npnpm_cache_dir={password='SECRET_SENTINEL'}\n",
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
