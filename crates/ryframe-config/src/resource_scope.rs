use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer};

/// 一个部署环境对共享基础设施资源的稳定所有权标识。
///
/// 该值同时进入 Redis 命名空间、对象键前缀和重建清单。限制为较短的 ASCII 标识，
/// 以避免不同基础设施对 Unicode、大小写和路径字符的解释不一致。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ResourceScopeId(String);

impl ResourceScopeId {
    pub const MIN_LEN: usize = 2;
    pub const MAX_LEN: usize = 48;

    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let bytes = value.as_bytes();
        let edge_is_alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
        if !(Self::MIN_LEN..=Self::MAX_LEN).contains(&bytes.len())
            || !bytes
                .first()
                .is_some_and(|byte| edge_is_alphanumeric(*byte))
            || !bytes.last().is_some_and(|byte| edge_is_alphanumeric(*byte))
            || !bytes.iter().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
        {
            return Err(format!(
                "scope_id 必须为 {}–{} 位小写 ASCII 字母、数字、下划线或连字符，且首尾必须是字母或数字",
                Self::MIN_LEN,
                Self::MAX_LEN
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Redis Cluster hash tag 也固定在 scope 上，确保扫描和批量事务不会跨环境。
    pub fn redis_namespace(&self) -> String {
        format!("ryframe:{{{}}}:", self.0)
    }

    /// 五类逻辑存储桶内统一使用该前缀。
    pub fn object_prefix(&self) -> String {
        format!("{}/", self.0)
    }

    pub fn ownership_marker(&self, resource_kind: &str) -> String {
        format!("ryframe-owner:v1:{}:{resource_kind}", self.0)
    }
}

impl AsRef<str> for ResourceScopeId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ResourceScopeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ResourceScopeId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl<'de> Deserialize<'de> for ResourceScopeId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::ResourceScopeId;

    #[test]
    fn scope_generates_stable_resource_namespaces() {
        let scope = ResourceScopeId::parse("dev_local-01").expect("作用域有效");
        assert_eq!(scope.redis_namespace(), "ryframe:{dev_local-01}:");
        assert_eq!(scope.object_prefix(), "dev_local-01/");
        assert_eq!(
            scope.ownership_marker("redis"),
            "ryframe-owner:v1:dev_local-01:redis"
        );
    }

    #[test]
    fn scope_rejects_ambiguous_or_path_like_values() {
        for invalid in ["a", "Dev", "dev local", "dev/local", "-dev", "dev-"] {
            assert!(ResourceScopeId::parse(invalid).is_err(), "{invalid}");
        }
    }
}
