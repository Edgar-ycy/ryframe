//! 功能开关（Feature Flag）
//!
//! 提供运行时动态开启/关闭功能的机制。
//! 支持从配置文件加载默认值，并通过 API 动态修改。
//!
//! # 使用示例
//!
//! ```ignore
//! use ryframe_core::feature_flag::FeatureFlags;
//!
//! let flags = FeatureFlags::new()
//!     .with_flag("new_user_flow", true, "新版用户注册流程")
//!     .with_flag("dark_mode", false, "深色模式");
//!
//! if flags.is_enabled("new_user_flow") {
//!     // 启用新功能
//! }
//! ```

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};

/// 单个功能开关定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureDefinition {
    /// 功能标识（唯一键）
    pub key: String,
    /// 功能描述
    pub description: String,
    /// 是否启用
    pub enabled: bool,
    /// 是否为系统保留（不允许运行时修改）
    pub system: bool,
}

/// 功能开关管理器
///
/// 线程安全，支持运行时动态修改非系统标志。
#[derive(Clone)]
pub struct FeatureFlags {
    inner: Arc<RwLock<HashMap<String, FeatureDefinition>>>,
}

impl FeatureFlags {
    /// 创建空的功能开关集合
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 从配置 map 创建（用于从配置文件中加载）
    pub fn from_config(config: &HashMap<String, bool>) -> Self {
        let flags = Self::new();
        for (key, enabled) in config {
            flags.register(key, *enabled, &format!("从配置加载: {}", key), false);
        }
        flags
    }

    /// 注册一个功能开关（如果已存在则跳过）
    fn register(&self, key: &str, enabled: bool, description: &str, system: bool) {
        let mut map = self.inner.write().unwrap();
        map.entry(key.to_string())
            .or_insert_with(|| FeatureDefinition {
                key: key.to_string(),
                description: description.to_string(),
                enabled,
                system,
            });
    }

    /// 添加一个功能开关（Builder 模式）
    pub fn with_flag(self, key: &str, enabled: bool, description: &str) -> Self {
        self.register(key, enabled, description, false);
        self
    }

    /// 添加一个系统保留标志（不能通过 API 动态修改）
    pub fn with_system_flag(self, key: &str, enabled: bool, description: &str) -> Self {
        self.register(key, enabled, description, true);
        self
    }

    /// 检查功能是否启用
    ///
    /// 未注册的功能默认返回 `default`（通常为 false）。
    pub fn is_enabled(&self, key: &str) -> bool {
        self.inner
            .read()
            .unwrap()
            .get(key)
            .map(|f| f.enabled)
            .unwrap_or(false)
    }

    /// 检查功能是否启用（带默认值）
    pub fn is_enabled_or(&self, key: &str, default: bool) -> bool {
        self.inner
            .read()
            .unwrap()
            .get(key)
            .map(|f| f.enabled)
            .unwrap_or(default)
    }

    /// 设置功能开关状态
    ///
    /// 返回 `true` 表示设置成功，对未注册或不存在的 key 返回 `false`。
    /// 系统保留标志不允许修改。
    pub fn set_enabled(&self, key: &str, enabled: bool) -> bool {
        let mut map = self.inner.write().unwrap();
        if let Some(feature) = map.get_mut(key) {
            if feature.system {
                tracing::warn!("系统保留标志不允许运行时修改: {}", key);
                return false;
            }
            feature.enabled = enabled;
            tracing::info!("功能开关已更新: {} = {}", key, enabled);
            true
        } else {
            false
        }
    }

    /// 切换功能开关（toggle）
    pub fn toggle(&self, key: &str) -> Option<bool> {
        let mut map = self.inner.write().unwrap();
        if let Some(feature) = map.get_mut(key) {
            if feature.system {
                return None;
            }
            feature.enabled = !feature.enabled;
            Some(feature.enabled)
        } else {
            None
        }
    }

    /// 获取所有功能开关
    pub fn list_all(&self) -> Vec<FeatureDefinition> {
        self.inner.read().unwrap().values().cloned().collect()
    }

    /// 获取指定功能开关详情
    pub fn get(&self, key: &str) -> Option<FeatureDefinition> {
        self.inner.read().unwrap().get(key).cloned()
    }

    /// 获取所有已启用（公开）的功能开关
    pub fn enabled_flags(&self) -> Vec<FeatureDefinition> {
        self.inner
            .read()
            .unwrap()
            .values()
            .filter(|f| f.enabled && !f.system)
            .cloned()
            .collect()
    }

    /// 导出为配置 map（用于持久化）
    pub fn export_config(&self) -> HashMap<String, bool> {
        self.inner
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.enabled))
            .collect()
    }
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for FeatureFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let map = self.inner.read().unwrap();
        f.debug_map()
            .entries(map.values().map(|v| (&v.key, v.enabled)))
            .finish()
    }
}

/// 功能开关的默认预设
pub struct FeaturePresets;

impl FeaturePresets {
    /// 标准预设（适用于一般生产环境）
    pub fn standard() -> FeatureFlags {
        FeatureFlags::new()
            .with_flag("user_registration", true, "用户注册")
            .with_flag("email_verification", true, "邮箱验证")
            .with_flag("captcha_enabled", true, "验证码")
            .with_flag("rate_limiting", true, "API限流")
            .with_flag("audit_logging", true, "操作审计")
            .with_flag("beta_features", false, "Beta功能")
            .with_flag("maintenance_mode", false, "维护模式")
            .with_system_flag("core_auth", true, "核心认证（不可关闭）")
    }

    /// 开发环境预设（宽松模式）
    pub fn development() -> FeatureFlags {
        FeatureFlags::new()
            .with_flag("user_registration", true, "用户注册")
            .with_flag("email_verification", false, "邮箱验证")
            .with_flag("captcha_enabled", false, "验证码")
            .with_flag("rate_limiting", false, "API限流")
            .with_flag("audit_logging", false, "操作审计")
            .with_flag("beta_features", true, "Beta功能")
            .with_flag("maintenance_mode", false, "维护模式")
            .with_system_flag("core_auth", true, "核心认证")
    }
}
