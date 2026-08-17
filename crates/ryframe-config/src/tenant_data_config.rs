use std::{collections::HashSet, fmt};

use serde::Deserialize;

use crate::DbTlsMode;

/// 复用控制库作为租户数据面的保留目标键。
pub const SHARED_CONTROL_TARGET_KEY: &str = "shared-control";
pub const MAX_TENANT_DATABASE_TARGETS: usize = 200;

/// 租户数据目标的占用方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantDatabaseTargetMode {
    /// 一个目标可以承载多个租户，但业务表仍必须显式按 `tenant_id` 隔离。
    Shared,
    /// 一个目标在任意时刻只能存在一个 active 租户 fence。
    Dedicated,
}

/// 租户数据目标的连接来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantDatabaseTargetKind {
    /// 复用组合根已经建立的控制库集群，不创建第二个连接池。
    Control,
    /// 使用目标自身的 MySQL 配置延迟建立连接池。
    Mysql,
}

/// 租户数据面路由及连接池总预算。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantDataConfig {
    /// 新租户 provision 时使用的默认目标；运行时缺失 placement 仍会 fail-closed。
    #[serde(default = "default_target")]
    pub default_target: String,
    /// 单个 API/Worker 进程最多同时缓存的独立 MySQL 目标池数量。
    #[serde(default = "default_max_open_targets")]
    pub max_open_targets: usize,
    /// 所有独立目标连接池 `max_connections` 之和的硬预算。
    #[serde(default = "default_max_total_connections")]
    pub max_total_connections: u32,
    /// 无活动 Session 的目标池在注册表中保留的秒数。
    #[serde(default = "default_idle_pool_secs")]
    pub idle_pool_secs: u64,
    /// 已批准的控制库与 MySQL 目标。省略 `shared-control` 时由规范化阶段注入。
    #[serde(default)]
    pub targets: Vec<TenantDatabaseTargetConfig>,
}

impl TenantDataConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.default_target != self.default_target.trim()
            || !is_valid_target_key(&self.default_target)
        {
            return Err("tenant_data.default_target 不是有效目标键".into());
        }
        if self.max_open_targets == 0 || self.max_open_targets > MAX_TENANT_DATABASE_TARGETS {
            return Err(format!(
                "tenant_data.max_open_targets 必须在 1 到 {MAX_TENANT_DATABASE_TARGETS} 之间"
            ));
        }
        if self.max_total_connections < self.max_open_targets as u32
            || self.max_total_connections > 100_000
        {
            return Err(
                "tenant_data.max_total_connections 必须不少于 max_open_targets 且不超过 100000"
                    .into(),
            );
        }
        if !(1..=86_400).contains(&self.idle_pool_secs) {
            return Err("tenant_data.idle_pool_secs 必须在 1 到 86400 之间".into());
        }
        let normalized_target_count = self.targets.len()
            + usize::from(
                !self
                    .targets
                    .iter()
                    .any(|target| target.key == SHARED_CONTROL_TARGET_KEY),
            );
        if normalized_target_count > MAX_TENANT_DATABASE_TARGETS {
            return Err(format!(
                "tenant_data.targets（含 shared-control）最多允许 {MAX_TENANT_DATABASE_TARGETS} 个目标"
            ));
        }

        let mut keys = HashSet::with_capacity(self.targets.len() + 1);
        let mut control_count = 0;
        for (index, target) in self.targets.iter().enumerate() {
            target.validate(index)?;
            if target
                .max_connections
                .is_some_and(|limit| limit > self.max_total_connections)
            {
                return Err(format!(
                    "tenant_data.targets[{index}].max_connections 不能超过 max_total_connections"
                ));
            }
            if !keys.insert(target.key.as_str()) {
                return Err(format!("tenant_data.targets 目标键重复: {}", target.key));
            }
            if target.kind == TenantDatabaseTargetKind::Control {
                control_count += 1;
            }
        }
        if control_count > 1 {
            return Err("tenant_data.targets 最多只能声明一个 control 目标".into());
        }
        if self.default_target != SHARED_CONTROL_TARGET_KEY
            && !keys.contains(self.default_target.as_str())
        {
            return Err(format!(
                "tenant_data.default_target 未注册: {}",
                self.default_target
            ));
        }
        Ok(())
    }

    /// 返回包含隐式 `shared-control` 的规范化目标集合。
    pub fn normalized_targets(&self) -> Vec<TenantDatabaseTargetConfig> {
        let mut targets = self.targets.clone();
        if !targets
            .iter()
            .any(|target| target.key == SHARED_CONTROL_TARGET_KEY)
        {
            targets.push(TenantDatabaseTargetConfig::shared_control());
        }
        targets
    }
}

impl Default for TenantDataConfig {
    fn default() -> Self {
        Self {
            default_target: default_target(),
            max_open_targets: default_max_open_targets(),
            max_total_connections: default_max_total_connections(),
            idle_pool_secs: default_idle_pool_secs(),
            targets: Vec::new(),
        }
    }
}

/// 一个可供控制面 placement 引用的租户数据目标。
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantDatabaseTargetConfig {
    pub key: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    pub mode: TenantDatabaseTargetMode,
    pub kind: TenantDatabaseTargetKind,
    #[serde(default)]
    pub max_connections: Option<u32>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password_env: Option<String>,
    #[serde(default)]
    pub tls_mode: Option<DbTlsMode>,
    #[serde(default)]
    pub tls_ca: Option<String>,
    #[serde(default)]
    pub tls_client_cert: Option<String>,
    #[serde(default)]
    pub tls_client_key: Option<String>,
}

impl fmt::Debug for TenantDatabaseTargetConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TenantDatabaseTargetConfig")
            .field("key", &self.key)
            .field("display_name", &self.display_name)
            .field("region", &self.region)
            .field("mode", &self.mode)
            .field("kind", &self.kind)
            .field("max_connections", &self.max_connections)
            .finish_non_exhaustive()
    }
}

impl TenantDatabaseTargetConfig {
    pub fn shared_control() -> Self {
        Self {
            key: SHARED_CONTROL_TARGET_KEY.into(),
            display_name: Some("共享控制库".into()),
            region: None,
            mode: TenantDatabaseTargetMode::Shared,
            kind: TenantDatabaseTargetKind::Control,
            max_connections: None,
            host: None,
            port: None,
            database: None,
            username: None,
            password_env: None,
            tls_mode: None,
            tls_ca: None,
            tls_client_cert: None,
            tls_client_key: None,
        }
    }

    fn validate(&self, index: usize) -> Result<(), String> {
        if self.key != self.key.trim() || !is_valid_target_key(&self.key) {
            return Err(format!(
                "tenant_data.targets[{index}].key 必须为 2–64 位 ASCII 字母、数字、下划线或连字符，且首尾必须是字母或数字"
            ));
        }
        for (field, value, max_bytes) in [
            ("display_name", self.display_name.as_deref(), 128),
            ("region", self.region.as_deref(), 64),
        ] {
            if let Some(value) = value
                && (value.trim().is_empty() || value.len() > max_bytes)
            {
                return Err(format!(
                    "tenant_data.targets[{index}].{field} 不能为空且不能超过 {max_bytes} 字节"
                ));
            }
        }
        if self.max_connections.is_some_and(|limit| limit == 0) {
            return Err(format!(
                "tenant_data.targets[{index}].max_connections 必须大于 0"
            ));
        }
        match self.kind {
            TenantDatabaseTargetKind::Control => self.validate_control(index),
            TenantDatabaseTargetKind::Mysql => self.validate_mysql(index),
        }
    }

    fn validate_control(&self, index: usize) -> Result<(), String> {
        if self.key != SHARED_CONTROL_TARGET_KEY || self.mode != TenantDatabaseTargetMode::Shared {
            return Err(format!(
                "tenant_data.targets[{index}] 的 control 目标必须使用 key=shared-control、mode=shared"
            ));
        }
        if self.host.is_some()
            || self.port.is_some()
            || self.database.is_some()
            || self.username.is_some()
            || self.password_env.is_some()
            || self.max_connections.is_some()
            || self.tls_mode.is_some()
            || self.tls_ca.is_some()
            || self.tls_client_cert.is_some()
            || self.tls_client_key.is_some()
        {
            return Err(format!(
                "tenant_data.targets[{index}] 的 control 目标不得声明 MySQL 连接字段"
            ));
        }
        Ok(())
    }

    fn validate_mysql(&self, index: usize) -> Result<(), String> {
        if self.key == SHARED_CONTROL_TARGET_KEY {
            return Err(format!(
                "tenant_data.targets[{index}] 的保留 key=shared-control 必须精确使用 mode=shared、kind=control"
            ));
        }
        let required = [
            ("host", self.host.as_deref()),
            ("database", self.database.as_deref()),
            ("username", self.username.as_deref()),
            ("password_env", self.password_env.as_deref()),
        ];
        if let Some((field, _)) = required
            .into_iter()
            .find(|(_, value)| value.is_none_or(|value| value.trim().is_empty()))
        {
            return Err(format!(
                "tenant_data.targets[{index}].{field} 是 mysql 目标必填项"
            ));
        }
        if self.port.is_some_and(|port| port == 0) {
            return Err(format!("tenant_data.targets[{index}].port 必须大于 0"));
        }
        let password_env = self.password_env.as_deref().unwrap_or_default();
        if !is_valid_environment_name(password_env) {
            return Err(format!(
                "tenant_data.targets[{index}].password_env 必须是有效环境变量名"
            ));
        }
        let client_cert = non_empty(self.tls_client_cert.as_deref());
        let client_key = non_empty(self.tls_client_key.as_deref());
        if client_cert.is_some() != client_key.is_some() {
            return Err(format!(
                "tenant_data.targets[{index}].tls_client_cert 和 tls_client_key 必须同时配置"
            ));
        }
        if matches!(
            self.tls_mode.unwrap_or_default(),
            DbTlsMode::VerifyCa | DbTlsMode::VerifyIdentity
        ) && non_empty(self.tls_ca.as_deref()).is_none()
        {
            return Err(format!(
                "tenant_data.targets[{index}].tls_ca 是证书验证模式必填项"
            ));
        }
        Ok(())
    }
}

const fn default_max_open_targets() -> usize {
    32
}

const fn default_max_total_connections() -> u32 {
    200
}

const fn default_idle_pool_secs() -> u64 {
    600
}

fn default_target() -> String {
    SHARED_CONTROL_TARGET_KEY.into()
}

pub fn is_valid_target_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    let is_alphanumeric = |byte: u8| byte.is_ascii_alphanumeric();
    (2..=64).contains(&bytes.len())
        && bytes.first().is_some_and(|byte| is_alphanumeric(*byte))
        && bytes.last().is_some_and(|byte| is_alphanumeric(*byte))
        && bytes
            .iter()
            .all(|byte| is_alphanumeric(*byte) || matches!(byte, b'-' | b'_'))
}

fn is_valid_environment_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes
        .first()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
