use std::collections::{BTreeMap, BTreeSet};

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

/// 纯内存语言资源不满足运行时约束。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalizationError {
    MissingLocale {
        locale: Locale,
    },
    KeyParity {
        locale: Locale,
        missing: Vec<String>,
        extra: Vec<String>,
    },
    PlaceholderParity {
        locale: Locale,
        key: String,
        missing: Vec<String>,
        extra: Vec<String>,
    },
}

impl std::fmt::Display for LocalizationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingLocale { locale } => write!(formatter, "缺少语言资源: {locale}"),
            Self::KeyParity {
                locale,
                missing,
                extra,
            } => write!(
                formatter,
                "语言资源键集合不一致，{locale} 缺少: {missing:?}；额外: {extra:?}"
            ),
            Self::PlaceholderParity {
                locale,
                key,
                missing,
                extra,
            } => write!(
                formatter,
                "语言资源占位符不一致，{locale} 的 {key} 缺少: {missing:?}；额外: {extra:?}"
            ),
        }
    }
}

impl std::error::Error for LocalizationError {}

/// 通过已校验的纯内存资源查询并渲染本地化文本。
#[derive(Debug, Clone)]
pub struct Localizer {
    resources: BTreeMap<Locale, BTreeMap<String, String>>,
}

impl Localizer {
    /// 从完整语言资源构造本地化器，并校验语言、键与占位符集合。
    pub fn from_resources(
        resources: BTreeMap<Locale, BTreeMap<String, String>>,
    ) -> Result<Self, LocalizationError> {
        for locale in Locale::ALL {
            if !resources.contains_key(&locale) {
                return Err(LocalizationError::MissingLocale { locale });
            }
        }

        let default_entries = resources
            .get(&Locale::DEFAULT)
            .expect("默认语言资源已在此前验证");
        let default_keys = default_entries.keys().cloned().collect::<BTreeSet<_>>();
        for locale in Locale::ALL {
            let entries = resources.get(&locale).expect("语言资源已在此前验证");
            let keys = entries.keys().cloned().collect::<BTreeSet<_>>();
            let missing = default_keys.difference(&keys).cloned().collect::<Vec<_>>();
            let extra = keys.difference(&default_keys).cloned().collect::<Vec<_>>();
            if !missing.is_empty() || !extra.is_empty() {
                return Err(LocalizationError::KeyParity {
                    locale,
                    missing,
                    extra,
                });
            }
        }

        for locale in Locale::ALL {
            let entries = resources.get(&locale).expect("语言资源已在此前验证");
            for (key, default_text) in default_entries {
                let default_placeholders = placeholders(default_text);
                let localized_placeholders =
                    placeholders(entries.get(key).expect("键集合已在此前验证"));
                let missing = default_placeholders
                    .difference(&localized_placeholders)
                    .cloned()
                    .collect::<Vec<_>>();
                let extra = localized_placeholders
                    .difference(&default_placeholders)
                    .cloned()
                    .collect::<Vec<_>>();
                if !missing.is_empty() || !extra.is_empty() {
                    return Err(LocalizationError::PlaceholderParity {
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

    pub fn render(&self, text: &LocalizedText, locale: Locale) -> String {
        match text {
            LocalizedText::Literal { value } => value.clone(),
            LocalizedText::Key { key, args } => self.translate_with_args(locale, key, args),
        }
    }

    pub fn has_key(&self, key: &str) -> bool {
        self.resources
            .get(&Locale::DEFAULT)
            .is_some_and(|entries| entries.contains_key(key))
    }
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
