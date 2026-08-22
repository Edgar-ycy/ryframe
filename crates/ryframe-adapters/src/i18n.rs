//! 本地化资源的文件、环境与内嵌加载适配器。

use std::{collections::BTreeMap, env, fs, path::Path};

use ryframe_kernel::{Locale, LocalizationError, Localizer};

/// 国际化资源加载或格式解析失败。
#[derive(Debug, thiserror::Error)]
pub enum I18nError {
    #[error("无法读取语言资源目录 {path}: {source}")]
    ReadDirectory {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("缺少语言资源文件: {path}")]
    MissingResource { path: String },
    #[error("无法读取语言资源文件 {path}: {source}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("语言资源文件 {path} 格式无效: {source}")]
    ParseFile {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("语言资源 {locale} 含有非字符串值: {key}")]
    NonStringValue { locale: Locale, key: String },
    #[error(transparent)]
    InvalidResources(#[from] LocalizationError),
}

/// 将外部或编译时内嵌资源加载为 kernel 本地化器。
pub struct LocalizerLoader;

impl LocalizerLoader {
    /// 从目录加载 `zh-CN.toml` 和 `en-US.toml`。
    pub fn load(locale_dir: impl AsRef<Path>) -> Result<Localizer, I18nError> {
        let locale_dir = locale_dir.as_ref();
        fs::read_dir(locale_dir).map_err(|source| I18nError::ReadDirectory {
            path: locale_dir.display().to_string(),
            source,
        })?;

        let mut resources = BTreeMap::new();
        for locale in Locale::ALL {
            let path = locale_dir.join(format!("{}.toml", locale.as_str()));
            let source = fs::read_to_string(&path).map_err(|source| {
                if source.kind() == std::io::ErrorKind::NotFound {
                    I18nError::MissingResource {
                        path: path.display().to_string(),
                    }
                } else {
                    I18nError::ReadFile {
                        path: path.display().to_string(),
                        source,
                    }
                }
            })?;
            resources.insert(locale, parse_resource(locale, &path, &source)?);
        }
        Ok(Localizer::from_resources(resources)?)
    }

    /// 从编译时内嵌资源创建本地化器，供开发与隔离环境使用。
    pub fn embedded() -> Result<Localizer, I18nError> {
        let mut resources = BTreeMap::new();
        resources.insert(
            Locale::ZhCn,
            parse_resource(
                Locale::ZhCn,
                Path::new("<embedded:zh-CN>"),
                include_str!("../../../locales/zh-CN.toml"),
            )?,
        );
        resources.insert(
            Locale::EnUs,
            parse_resource(
                Locale::EnUs,
                Path::new("<embedded:en-US>"),
                include_str!("../../../locales/en-US.toml"),
            )?,
        );
        Ok(Localizer::from_resources(resources)?)
    }

    /// 从 `APP_LOCALES_DIR` 或默认 `locales` 目录加载资源。
    ///
    /// 严格模式下缺失资源会直接失败；非严格模式可回退到内嵌资源，保证本地启动体验。
    pub fn load_from_environment(strict_resource_loading: bool) -> Result<Localizer, I18nError> {
        let locale_dir = env::var("APP_LOCALES_DIR").unwrap_or_else(|_| "locales".to_owned());
        match Self::load(&locale_dir) {
            Ok(localizer) => Ok(localizer),
            Err(error) if !strict_resource_loading => Self::embedded().or(Err(error)),
            Err(error) => Err(error),
        }
    }
}

fn parse_resource(
    locale: Locale,
    path: &Path,
    source: &str,
) -> Result<BTreeMap<String, String>, I18nError> {
    let value = toml::from_str::<toml::Value>(source).map_err(|source| I18nError::ParseFile {
        path: path.display().to_string(),
        source,
    })?;
    let mut entries = BTreeMap::new();
    flatten_resource(locale, "", &value, &mut entries)?;
    Ok(entries)
}

fn flatten_resource(
    locale: Locale,
    prefix: &str,
    value: &toml::Value,
    entries: &mut BTreeMap<String, String>,
) -> Result<(), I18nError> {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                let nested_key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_resource(locale, &nested_key, value, entries)?;
            }
            Ok(())
        }
        toml::Value::String(value) => {
            entries.insert(prefix.to_owned(), value.clone());
            Ok(())
        }
        _ => Err(I18nError::NonStringValue {
            locale,
            key: prefix.to_owned(),
        }),
    }
}
