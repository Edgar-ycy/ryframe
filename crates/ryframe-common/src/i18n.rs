//! 国际化 i18n 支持
//!
//! 提供多语言翻译能力：
//! - TOML 格式翻译资源文件加载（`locales/zh-CN.toml`, `locales/en-US.toml`...）
//! - 基于 `Accept-Language` 请求头的语言检测
//! - 翻译键嵌套查找（`error.not_found` → 按 `.` 分隔递归查找）
//! - 参数化翻译（`"Hello {name}"` 替换占位符）
//! - Axum 中间件：自动注入翻译函数到请求 extensions
//!
//! # 使用示例
//!
//! ```
//! use ryframe_common::i18n::{I18nManager, detect_language};
//!
//! // 创建管理器（不加载文件，使用内置翻译）
//! let i18n = I18nManager::load("nonexistent_dir").unwrap();
//! assert_eq!(i18n.default_lang(), "zh-CN");
//!
//! // 翻译回退到 key 本身
//! let msg = i18n.translate("unknown.key", "zh-CN");
//! assert_eq!(msg, "unknown.key");
//!
//! // 带参数翻译
//! let msg = i18n.translate_with_args("Hello, {name}!", "zh-CN", &[("name", "Alice")]);
//! assert_eq!(msg, "Hello, Alice!");
//!
//! // 语言检测
//! let lang = detect_language(Some("en-US,en;q=0.9"), &["zh-CN".into(), "en-US".into()]);
//! assert_eq!(lang, "en-us");
//! ```

use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use dashmap::DashMap;
use serde::Deserialize;
use tracing::{info, warn};

// ============ 全局实例 ============

static GLOBAL_I18N: OnceLock<I18nManager> = OnceLock::new();

/// 获取全局 I18nManager（需先调用 `set_global()`）
pub fn global_i18n() -> Option<&'static I18nManager> {
    GLOBAL_I18N.get()
}

// ============ 翻译资源 ============

/// 翻译资源（从 TOML 文件反序列化）
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LocaleResources {
    #[serde(flatten)]
    entries: toml::Table,
}

/// 国际化管理器
#[derive(Debug, Clone)]
pub struct I18nManager {
    /// 语言代码 → 翻译资源
    resources: Arc<DashMap<String, LocaleResources>>,
    /// 默认语言
    default_lang: String,
    /// 支持的语言列表
    supported_langs: Vec<String>,
}

impl I18nManager {
    /// 从目录加载所有翻译文件
    ///
    /// 扫描指定目录中的 `*.toml` 文件，文件名（不含扩展名）作为语言代码。
    /// 例如：`locales/zh-CN.toml` → 语言代码 `"zh-CN"`。
    pub fn load(locale_dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = locale_dir.as_ref();
        let resources = Arc::new(DashMap::new());
        let mut supported_langs = Vec::new();

        if !dir.exists() {
            warn!("locales 目录不存在: {}，使用内置翻译", dir.display());
            return Ok(Self {
                resources,
                default_lang: "zh-CN".into(),
                supported_langs: vec!["zh-CN".into(), "en-US".into()],
            });
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                return Err(format!("无法读取 locales 目录: {}", e));
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "toml") {
                let lang = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                match std::fs::read_to_string(&path) {
                    Ok(content) => match toml::from_str::<LocaleResources>(&content) {
                        Ok(locale) => {
                            info!(lang = %lang, file = %path.display(), "翻译文件已加载");
                            supported_langs.push(lang.clone());
                            resources.insert(lang, locale);
                        }
                        Err(e) => {
                            warn!(lang = %lang, error = %e, "翻译文件解析失败，跳过");
                        }
                    },
                    Err(e) => {
                        warn!(lang = %lang, error = %e, "翻译文件读取失败，跳过");
                    }
                }
            }
        }

        let default_lang = if supported_langs.contains(&"zh-CN".to_string()) {
            "zh-CN".into()
        } else if !supported_langs.is_empty() {
            supported_langs[0].clone()
        } else {
            "zh-CN".into()
        };

        info!(
            default_lang = %default_lang,
            supported = ?supported_langs,
            "国际化初始化完成"
        );

        Ok(Self {
            resources,
            default_lang,
            supported_langs,
        })
    }

    /// 设为全局实例
    pub fn set_global(self) {
        let _ = GLOBAL_I18N.set(self);
    }

    /// 获取默认语言
    pub fn default_lang(&self) -> &str {
        &self.default_lang
    }

    /// 获取支持的语言列表
    pub fn supported_langs(&self) -> &[String] {
        &self.supported_langs
    }

    /// 翻译键值
    ///
    /// 键使用 `.` 分隔，支持嵌套查找。
    /// 例如：`"error.not_found"` 会查找 `resources[lang].error.not_found`。
    ///
    /// 如果语言不存在或键不存在，回退到默认语言，最后返回键本身。
    pub fn translate(&self, key: &str, lang: &str) -> String {
        // 尝试请求语言
        if let Some(result) = self.lookup(key, lang) {
            return result;
        }

        // 回退到默认语言
        if lang != self.default_lang
            && let Some(result) = self.lookup(key, &self.default_lang)
        {
            return result;
        }

        // 回退到任意已加载语言
        for entry in self.resources.iter() {
            if entry.key() != lang
                && entry.key() != &self.default_lang
                && let Some(result) = self.lookup_in_resource(key, entry.value())
            {
                return result;
            }
        }

        // 最终回退：返回键本身
        key.to_string()
    }

    /// 带参数翻译
    ///
    /// 替换占位符 `{name}` 为对应值。
    ///
    /// # 示例
    /// ```
    /// # use ryframe_common::i18n::I18nManager;
    /// let i18n = I18nManager::load("nonexistent_dir").unwrap();
    /// let msg = i18n.translate_with_args("Hello, {name}!", "zh-CN", &[("name", "Alice")]);
    /// assert_eq!(msg, "Hello, Alice!");
    /// ```
    pub fn translate_with_args(&self, key: &str, lang: &str, args: &[(&str, &str)]) -> String {
        let template = self.translate(key, lang);
        let mut result = template;
        for (name, value) in args {
            let placeholder = format!("{{{}}}", name);
            result = result.replace(&placeholder, value);
        }
        result
    }

    /// 检查语言是否被支持
    pub fn is_supported(&self, lang: &str) -> bool {
        self.supported_langs.contains(&lang.to_string())
    }

    // ============ 内部方法 ============

    fn lookup(&self, key: &str, lang: &str) -> Option<String> {
        self.resources
            .get(lang)
            .and_then(|r| self.lookup_in_resource(key, &r))
    }

    fn lookup_in_resource(&self, key: &str, resource: &LocaleResources) -> Option<String> {
        let parts: Vec<&str> = key.split('.').collect();
        let mut current = toml::Value::Table(resource.entries.clone());

        for part in &parts {
            match current {
                toml::Value::Table(ref table) => {
                    current = table.get(*part)?.clone();
                }
                _ => return None,
            }
        }

        match current {
            toml::Value::String(s) => Some(s),
            toml::Value::Integer(i) => Some(i.to_string()),
            toml::Value::Float(f) => Some(f.to_string()),
            toml::Value::Boolean(b) => Some(b.to_string()),
            _ => None,
        }
    }
}

// ============ 便捷函数 ============

/// 翻译（使用全局 I18nManager）
///
/// 在 Handler 中直接调用，无需传递 I18nManager。
///
/// # 示例
/// ```
/// use ryframe_common::i18n::{I18nManager, translate};
///
/// // 初始化全局管理器
/// let i18n = I18nManager::load("nonexistent_dir").unwrap();
/// i18n.set_global();
///
/// // 翻译（key 不存在时回退到 key 本身）
/// let msg = translate("common.success", "zh-CN");
/// assert_eq!(msg, "common.success");
/// ```
pub fn translate(key: &str, lang: &str) -> String {
    global_i18n()
        .map(|i18n| i18n.translate(key, lang))
        .unwrap_or_else(|| key.to_string())
}

/// 带参数翻译（使用全局 I18nManager）
pub fn translate_with_args(key: &str, lang: &str, args: &[(&str, &str)]) -> String {
    global_i18n()
        .map(|i18n| i18n.translate_with_args(key, lang, args))
        .unwrap_or_else(|| key.to_string())
}

// ============ 语言检测 ============

/// 从 Accept-Language 请求头解析首选语言
///
/// 解析格式：`zh-CN,zh;q=0.9,en;q=0.8`
/// 返回最高 q 值的语言代码。
pub fn detect_language(accept_language: Option<&str>, supported: &[String]) -> String {
    let header = match accept_language {
        Some(h) => h,
        None => {
            return if supported.is_empty() {
                "zh-CN".into()
            } else {
                supported[0].clone()
            };
        }
    };

    // 解析 Accept-Language
    let mut langs: Vec<(String, f32)> = Vec::new();

    for part in header.split(',') {
        let part = part.trim();
        if let Some((lang_part, q_part)) = part.split_once(';') {
            let lang = lang_part.trim().to_lowercase();
            let q: f32 = q_part
                .trim()
                .strip_prefix("q=")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0);
            langs.push((lang, q));
        } else {
            langs.push((part.to_lowercase(), 1.0));
        }
    }

    // 按 q 值降序排列
    langs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    for (lang, _q) in &langs {
        // 精确匹配
        if supported.iter().any(|s| s.eq_ignore_ascii_case(lang)) {
            return lang.clone();
        }
        // 前缀匹配（如 zh 匹配 zh-CN）
        if let Some(prefix) = lang.split('-').next() {
            for s in supported {
                if s.starts_with(prefix) {
                    return s.clone();
                }
            }
        }
    }

    supported.first().cloned().unwrap_or_else(|| "zh-CN".into())
}

// ============ 内置翻译（默认加载） ============

/// 获取内置中文翻译（当翻译文件不存在时的回退）
pub fn builtin_zh_cn() -> LocaleResources {
    toml::from_str(include_str!("../../../locales/zh-CN.toml")).unwrap_or_default()
}

/// 获取内置英文翻译（当翻译文件不存在时的回退）
pub fn builtin_en_us() -> LocaleResources {
    toml::from_str(include_str!("../../../locales/en-US.toml")).unwrap_or_default()
}
