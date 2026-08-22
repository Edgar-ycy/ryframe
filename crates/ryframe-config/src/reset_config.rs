use serde::Deserialize;

/// 非生产全环境重建的额外所有权证明。
///
/// 三个独占开关只用于接管缺少新 scope marker 的旧资源；正常带 marker 的资源无需开启。
/// 所有开关默认关闭，生产环境即使显式开启也会被配置校验拒绝。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetConfig {
    #[serde(default)]
    pub legacy_mysql_exclusive: bool,
    #[serde(default)]
    pub legacy_redis_exclusive: bool,
    #[serde(default)]
    pub legacy_object_storage_exclusive: bool,
    /// 位于当前 namespace 外、由运维预先创建的 Redis 哨兵键。
    ///
    /// reset 只读取并比较它，绝不会创建、更新或删除它。
    #[serde(default)]
    pub redis_outside_sentinel_key: Option<String>,
    /// 由运维维护的非秘密凭据版本；任一 reset 相关密码或私钥轮换时必须同步变更。
    #[serde(default)]
    pub credential_version: Option<String>,
}

impl ResetConfig {
    pub fn validate(&self, production: bool) -> Result<(), String> {
        if production
            && (self.legacy_mysql_exclusive
                || self.legacy_redis_exclusive
                || self.legacy_object_storage_exclusive)
        {
            return Err("生产环境禁止声明 reset 旧资源独占权限".into());
        }
        if let Some(key) = self.redis_outside_sentinel_key.as_deref()
            && (key != key.trim()
                || key.is_empty()
                || key.len() > 512
                || key.chars().any(char::is_control))
        {
            return Err(
                "reset.redis_outside_sentinel_key 必须为 1–512 字节且不含空白边界或控制字符".into(),
            );
        }
        if let Some(version) = self.credential_version.as_deref()
            && (version != version.trim()
                || version.is_empty()
                || version.len() > 64
                || !version
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
        {
            return Err(
                "reset.credential_version 必须为 1–64 位 ASCII 字母、数字、点、下划线或连字符"
                    .into(),
            );
        }
        Ok(())
    }
}
