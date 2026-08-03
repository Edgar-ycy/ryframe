//! 仅通过 URL 进行 API 版本路由。
//!
//! RyFrame 有意不通过请求头协商版本：公开版本始终体现在路径中（`/api/v1`）。
//! 这可避免缓存歧义，并使 OpenAPI、限流和审计路径保持一致。

use std::{collections::BTreeMap, fmt, str::FromStr};

use axum::Router;
use ryframe_http::API_PREFIX;
use serde::{Deserialize, Serialize};

/// URL 中的 API 主版本（`v1`、`v2` 等）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ApiVersion {
    pub major: u32,
}

impl ApiVersion {
    pub const fn new(major: u32) -> Self {
        Self { major }
    }

    pub const fn v1() -> Self {
        Self { major: 1 }
    }

    pub const fn v2() -> Self {
        Self { major: 2 }
    }

    pub const fn v3() -> Self {
        Self { major: 3 }
    }

    pub fn path_prefix(&self) -> String {
        if self.major == 1 {
            API_PREFIX.to_owned()
        } else {
            format!("/api/v{}", self.major)
        }
    }

    pub fn from_path(path: &str) -> Option<Self> {
        let mut segments = path.split('/').filter(|segment| !segment.is_empty());
        if segments.next()? != "api" {
            return None;
        }
        let major = segments.next()?.strip_prefix('v')?.parse().ok()?;
        Some(Self::new(major))
    }

    pub const fn matches(&self, target: &ApiVersion) -> bool {
        self.major == target.major
    }

    pub fn all_supported() -> Vec<Self> {
        vec![Self::v1()]
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "v{}", self.major)
    }
}

impl FromStr for ApiVersion {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let major = value
            .trim()
            .trim_start_matches('v')
            .parse()
            .map_err(|_| format!("invalid API version: {value}"))?;
        Ok(Self::new(major))
    }
}

impl Default for ApiVersion {
    fn default() -> Self {
        Self::v1()
    }
}

/// 在显式 URL 版本前缀下构建路由树。
#[derive(Clone)]
pub struct VersionedRouter<S = ()>
where
    S: Clone + Send + Sync + 'static,
{
    versions: BTreeMap<ApiVersion, Router<S>>,
    latest: ApiVersion,
}

impl<S> VersionedRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            versions: BTreeMap::new(),
            latest: ApiVersion::v1(),
        }
    }

    pub fn with_version(mut self, version: ApiVersion, router: Router<S>) -> Self {
        if version.major > self.latest.major {
            self.latest = version.clone();
        }
        self.versions.insert(version, router);
        self
    }

    pub fn with_v1(self, router: Router<S>) -> Self {
        self.with_version(ApiVersion::v1(), router)
    }

    pub fn with_v2(self, router: Router<S>) -> Self {
        self.with_version(ApiVersion::v2(), router)
    }

    pub fn nest_version(mut self, version: ApiVersion, path: &str, router: Router<S>) -> Self {
        let existing = self.versions.remove(&version).unwrap_or_default();
        if version.major > self.latest.major {
            self.latest = version.clone();
        }
        self.versions.insert(version, existing.nest(path, router));
        self
    }

    pub fn latest_version(&self) -> &ApiVersion {
        &self.latest
    }

    pub fn registered_versions(&self) -> Vec<&ApiVersion> {
        self.versions.keys().collect()
    }

    pub fn has_version(&self, version: &ApiVersion) -> bool {
        self.versions.contains_key(version)
    }

    pub fn into_router(self) -> Router<S> {
        self.versions
            .into_iter()
            .fold(Router::new(), |root, (version, router)| {
                root.nest(version.path_prefix().as_str(), router)
            })
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
