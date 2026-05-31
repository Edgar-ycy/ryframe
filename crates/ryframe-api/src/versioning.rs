//! API 版本化管理
//!
//! 提供多版本路由注册机制，支持同时注册 v1/v2 等不同版本的 API。
//!
//! # 使用示例
//!
//! ```ignore
//! use ryframe_api::versioning::{ApiVersion, VersionedRouter};
//!
//! let router = VersionedRouter::new()
//!     .with_v1(v1_routes)
//!     .with_version(ApiVersion::v2(), v2_routes)
//!     .into_router(state);
//! ```

use std::{collections::BTreeMap, fmt, str::FromStr};

use axum::Router;
use serde::{Deserialize, Serialize};

/// API 版本号
///
/// 格式：`v{major}`，如 `v1`、`v2`。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ApiVersion {
    /// 主版本号
    pub major: u32,
}

impl ApiVersion {
    /// 创建 API 版本
    pub fn new(major: u32) -> Self {
        Self { major }
    }

    /// 常用版本快捷方式
    pub fn v1() -> Self {
        Self { major: 1 }
    }

    /// v2
    pub fn v2() -> Self {
        Self { major: 2 }
    }

    /// v3
    pub fn v3() -> Self {
        Self { major: 3 }
    }

    /// 转换为 URL 路径前缀，如 `/api/v1`
    pub fn path_prefix(&self) -> String {
        format!("/api/v{}", self.major)
    }

    /// 从路径中提取版本号
    ///
    /// 匹配路径中 `/api/v{数字}` 的模式。
    ///
    /// # Examples
    ///
    /// ```
    /// use ryframe_api::versioning::ApiVersion;
    ///
    /// assert_eq!(ApiVersion::from_path("/api/v1/users"), Some(ApiVersion::v1()));
    /// assert_eq!(ApiVersion::from_path("/api/v2/orders"), Some(ApiVersion::v2()));
    /// assert_eq!(ApiVersion::from_path("/other/path"), None);
    /// ```
    pub fn from_path(path: &str) -> Option<Self> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segments.len() >= 2 && segments[0] == "api" {
            let version_seg = segments[1];
            if let Some(num_str) = version_seg.strip_prefix('v')
                && let Ok(major) = num_str.parse::<u32>()
            {
                return Some(Self { major });
            }
        }
        None
    }

    /// 是否匹配给定的版本约束
    ///
    /// 当前仅支持精确匹配。
    pub fn matches(&self, target: &ApiVersion) -> bool {
        self.major == target.major
    }

    /// 版本列表（按升序排列）
    pub fn all_supported() -> Vec<Self> {
        vec![Self::v1()]
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.major)
    }
}

impl FromStr for ApiVersion {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim().trim_start_matches('v');
        let major = trimmed
            .parse::<u32>()
            .map_err(|_| format!("无效的版本号: {}", s))?;
        Ok(Self { major })
    }
}

/// 默认版本：v1
impl Default for ApiVersion {
    fn default() -> Self {
        Self::v1()
    }
}

/// 多版本 API 路由器
///
/// 管理多个 API 版本的路由，最终合并为一个 [`Router`]。
///
/// # 版本路由规则
///
/// - 每个版本注册在 `/api/v{major}` 路径前缀下
/// - 可通过 `nest_version` 为某版本添加子路由
/// - 通过 `into_router` 合并所有版本
#[derive(Clone)]
pub struct VersionedRouter<S = ()>
where
    S: Clone + Send + Sync + 'static,
{
    /// 版本 → 路由映射
    versions: BTreeMap<ApiVersion, Router<S>>,
    /// 最新版本（未匹配版本时默认使用）
    latest: ApiVersion,
}

impl<S> VersionedRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    /// 创建空的版本化路由器，默认最新版本为 v1
    pub fn new() -> Self {
        Self {
            versions: BTreeMap::new(),
            latest: ApiVersion::v1(),
        }
    }

    /// 为指定版本注册路由
    pub fn with_version(mut self, version: ApiVersion, router: Router<S>) -> Self {
        self.versions.insert(version.clone(), router);
        // 更新最新版本
        if version.major > self.latest.major {
            self.latest = version;
        }
        self
    }

    /// 注册 v1 路由的快捷方法
    pub fn with_v1(self, router: Router<S>) -> Self {
        self.with_version(ApiVersion::v1(), router)
    }

    /// 注册 v2 路由的快捷方法
    pub fn with_v2(self, router: Router<S>) -> Self {
        self.with_version(ApiVersion::v2(), router)
    }

    /// 在指定版本下嵌套子路由
    pub fn nest_version(mut self, version: ApiVersion, path: &str, router: Router<S>) -> Self {
        let existing = self
            .versions
            .remove(&version)
            .unwrap_or_else(|| Router::new());
        self.versions
            .insert(version.clone(), existing.nest(path, router));
        // update latest
        if version.major > self.latest.major {
            self.latest = version;
        }
        self
    }

    /// 获取最新版本号
    pub fn latest_version(&self) -> &ApiVersion {
        &self.latest
    }

    /// 获取所有已注册版本
    pub fn registered_versions(&self) -> Vec<&ApiVersion> {
        self.versions.keys().collect()
    }

    /// 检查版本是否已注册
    pub fn has_version(&self, version: &ApiVersion) -> bool {
        self.versions.contains_key(version)
    }

    /// 合并所有版本路由为单个 Router
    ///
    /// 每个版本映射到 `/api/v{major}` 前缀。
    pub fn into_router(self) -> Router<S> {
        let mut root = Router::new();
        for (version, router) in self.versions {
            root = root.nest(&version.path_prefix(), router);
        }
        root
    }
}

impl<S> Default for VersionedRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

// ========== 请求头版本协商 ==========

/// 从请求头中解析 API 版本
///
/// 支持以下来源（优先级从高到低）：
/// 1. `X-API-Version` 请求头
/// 2. `Accept-Version` 请求头
/// 3. URL 路径中的 `/api/v{n}` 前缀
///
/// 若均未指定，返回默认版本 v1。
pub struct VersionNegotiator;

impl VersionNegotiator {
    /// 从请求头解析版本
    ///
    /// # Arguments
    /// - `headers`: HTTP 请求头迭代器
    /// - `default`: 未指定时的默认版本
    pub fn from_headers(headers: &axum::http::HeaderMap, default: ApiVersion) -> ApiVersion {
        // 1. X-API-Version
        if let Some(val) = headers.get("X-API-Version")
            && let Ok(v) = val.to_str()
            && let Ok(version) = v.parse::<ApiVersion>()
        {
            return version;
        }

        // 2. Accept-Version
        if let Some(val) = headers.get("Accept-Version")
            && let Ok(v) = val.to_str()
            && let Ok(version) = v.parse::<ApiVersion>()
        {
            return version;
        }

        default
    }

    /// 从请求 URI 路径解析版本
    ///
    /// 如果路径中包含 `/api/v{n}`，则返回对应版本；否则返回 default。
    pub fn from_uri(uri: &axum::http::Uri, default: ApiVersion) -> ApiVersion {
        ApiVersion::from_path(uri.path()).unwrap_or(default)
    }
}
