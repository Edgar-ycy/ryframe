//! 显式注入的国际化基础设施。
//!
//! 该库只提供语言协商、资源校验和文本渲染，不依赖 HTTP、数据库或全局单例。

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::Path,
};

use serde::{Deserialize, Serialize};

/// 目前稳定支持的语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Locale {
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en-US")]
    EnUs,
}

impl Locale {
    pub const ALL: [Self; 2] = [Self::ZhCn, Self::EnUs];
    pub const DEFAULT: Self = Self::ZhCn;

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::EnUs => "en-US",
        }
    }

    /// 将常见的语言标签规范化到受支持语言。
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().replace('_', "-").to_ascii_lowercase();
        match normalized.as_str() {
            "zh" | "zh-cn" | "zh-hans" => Some(Self::ZhCn),
            "en" | "en-us" | "en-gb" => Some(Self::EnUs),
            _ => None,
        }
    }
}

impl std::fmt::Display for Locale {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 可持久化或可传输的本地化文本表达。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalizedText {
    /// 由管理员或业务方直接提供的文本，不作词典替换。
    Literal { value: String },
    /// 通过资源键与具名参数在当前语言中渲染的文本。
    Key {
        key: String,
        #[serde(default)]
        args: BTreeMap<String, String>,
    },
}

impl LocalizedText {
    pub fn render(&self, localizer: &Localizer, locale: Locale) -> String {
        match self {
            Self::Literal { value } => value.clone(),
            Self::Key { key, args } => localizer.translate_with_args(locale, key, args),
        }
    }
}

/// 国际化资源加载或一致性校验失败。
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
    #[error("语言资源键集合不一致，{locale} 缺少: {missing:?}；额外: {extra:?}")]
    KeyParity {
        locale: Locale,
        missing: Vec<String>,
        extra: Vec<String>,
    },
    #[error("语言资源占位符不一致，{locale} 的 {key} 缺少: {missing:?}；额外: {extra:?}")]
    PlaceholderParity {
        locale: Locale,
        key: String,
        missing: Vec<String>,
        extra: Vec<String>,
    },
}

/// 通过资源键查询并渲染本地化文本的无状态对象。
#[derive(Debug, Clone)]
pub struct Localizer {
    resources: BTreeMap<Locale, BTreeMap<String, String>>,
}

impl Localizer {
    /// 从目录加载 `zh-CN.toml` 和 `en-US.toml`，并在启动前校验键集合完全一致。
    pub fn load(locale_dir: impl AsRef<Path>) -> Result<Self, I18nError> {
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
        Self::from_resources(resources)
    }

    /// 从编译时内嵌资源创建本地化器，供开发与测试环境使用。
    pub fn embedded() -> Result<Self, I18nError> {
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
        Self::from_resources(resources)
    }

    /// 从 `APP_LOCALES_DIR` 或默认 `locales` 目录加载资源。
    ///
    /// 严格模式下缺失资源会直接失败；非严格模式可回退到内嵌资源，保证本地启动体验。
    pub fn load_from_environment(strict_resource_loading: bool) -> Result<Self, I18nError> {
        let locale_dir = env::var("APP_LOCALES_DIR").unwrap_or_else(|_| "locales".to_owned());
        match Self::load(&locale_dir) {
            Ok(localizer) => Ok(localizer),
            Err(error) if !strict_resource_loading => Self::embedded().or(Err(error)),
            Err(error) => Err(error),
        }
    }

    pub fn translate(&self, locale: Locale, key: &str) -> String {
        self.resources
            .get(&locale)
            .and_then(|entries| entries.get(key))
            .or_else(|| {
                self.resources
                    .get(&Locale::DEFAULT)
                    .and_then(|entries| entries.get(key))
            })
            .cloned()
            .unwrap_or_else(|| key.to_owned())
    }

    pub fn translate_with_args(
        &self,
        locale: Locale,
        key: &str,
        args: &BTreeMap<String, String>,
    ) -> String {
        args.iter()
            .fold(self.translate(locale, key), |text, (name, value)| {
                text.replace(&format!("{{{name}}}"), value)
            })
    }

    pub fn has_key(&self, key: &str) -> bool {
        self.resources
            .get(&Locale::DEFAULT)
            .is_some_and(|entries| entries.contains_key(key))
    }

    fn from_resources(
        resources: BTreeMap<Locale, BTreeMap<String, String>>,
    ) -> Result<Self, I18nError> {
        let default_keys = resources
            .get(&Locale::DEFAULT)
            .map(|entries| entries.keys().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        for locale in Locale::ALL {
            let keys = resources
                .get(&locale)
                .map(|entries| entries.keys().cloned().collect::<BTreeSet<_>>())
                .unwrap_or_default();
            let missing = default_keys.difference(&keys).cloned().collect::<Vec<_>>();
            let extra = keys.difference(&default_keys).cloned().collect::<Vec<_>>();
            if !missing.is_empty() || !extra.is_empty() {
                return Err(I18nError::KeyParity {
                    locale,
                    missing,
                    extra,
                });
            }
        }
        let default_entries = resources
            .get(&Locale::DEFAULT)
            .expect("默认语言资源已在加载时插入");
        for locale in Locale::ALL {
            let entries = resources
                .get(&locale)
                .expect("每种受支持语言的资源已在加载时插入");
            for (key, default_text) in default_entries {
                let default_placeholders = placeholders(default_text);
                let localized_placeholders =
                    placeholders(entries.get(key).expect("键集合一致性已在此前验证"));
                let missing = default_placeholders
                    .difference(&localized_placeholders)
                    .cloned()
                    .collect::<Vec<_>>();
                let extra = localized_placeholders
                    .difference(&default_placeholders)
                    .cloned()
                    .collect::<Vec<_>>();
                if !missing.is_empty() || !extra.is_empty() {
                    return Err(I18nError::PlaceholderParity {
                        locale,
                        key: key.clone(),
                        missing,
                        extra,
                    });
                }
            }
        }
        Ok(Self { resources })
    }
}

/// 优先按 `Accept-Language` 协商；没有可用值时再使用用户偏好；最终回退中文。
pub fn negotiate_locale(accept_language: Option<&str>, preferred_locale: Option<&str>) -> Locale {
    accept_language
        .and_then(parse_accept_language)
        .or_else(|| preferred_locale.and_then(Locale::parse))
        .unwrap_or(Locale::DEFAULT)
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

fn parse_accept_language(header: &str) -> Option<Locale> {
    let mut candidates = header
        .split(',')
        .filter_map(|item| {
            let mut parts = item.trim().split(';');
            let language = parts.next()?.trim();
            let quality = parts
                .find_map(|part| part.trim().strip_prefix("q="))
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(1.0);
            (quality > 0.0).then_some((Locale::parse(language), quality))
        })
        .filter_map(|(locale, quality)| locale.map(|locale| (locale, quality)))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.1.total_cmp(&left.1));
    candidates.first().map(|(locale, _)| *locale)
}

fn placeholders(text: &str) -> BTreeSet<String> {
    let mut placeholders = BTreeSet::new();
    let mut remaining = text;
    while let Some(start) = remaining.find('{') {
        remaining = &remaining[start + 1..];
        let Some(end) = remaining.find('}') else {
            break;
        };
        let name = remaining[..end].trim();
        if !name.is_empty()
            && name
                .bytes()
                .all(|character| character.is_ascii_alphanumeric() || character == b'_')
        {
            placeholders.insert(name.to_owned());
        }
        remaining = &remaining[end + 1..];
    }
    placeholders
}

#[cfg(test)]
mod tests {
    use super::{I18nError, Locale, LocalizedText, Localizer, negotiate_locale};
    use std::collections::BTreeMap;

    #[test]
    fn embedded_resources_have_identical_keys() {
        assert!(Localizer::embedded().is_ok());
    }

    #[test]
    fn accept_language_has_priority_over_user_preference() {
        assert_eq!(
            negotiate_locale(Some("en-GB,en;q=0.9"), Some("zh-CN")),
            Locale::EnUs
        );
        assert_eq!(negotiate_locale(Some("fr-FR"), Some("en-US")), Locale::EnUs);
        assert_eq!(negotiate_locale(None, None), Locale::ZhCn);
    }

    #[test]
    fn localized_key_renders_named_arguments() {
        let localizer = Localizer::embedded().expect("embedded resources");
        let text = LocalizedText::Key {
            key: "user.welcome".into(),
            args: BTreeMap::from([("name".into(), "Ada".into())]),
        };
        assert_eq!(text.render(&localizer, Locale::EnUs), "Welcome Ada");
    }

    #[test]
    fn resources_require_matching_named_placeholders() {
        let resources = BTreeMap::from([
            (
                Locale::ZhCn,
                BTreeMap::from([("user.welcome".into(), "你好，{name}".into())]),
            ),
            (
                Locale::EnUs,
                BTreeMap::from([("user.welcome".into(), "Welcome {account}".into())]),
            ),
        ]);

        assert!(matches!(
            Localizer::from_resources(resources),
            Err(I18nError::PlaceholderParity { .. })
        ));
    }
}
