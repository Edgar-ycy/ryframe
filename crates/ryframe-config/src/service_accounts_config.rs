use std::{collections::BTreeMap, fs, path::Path};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;

const MAX_PEPPER_VERSIONS: usize = 8;
const MIN_PEPPER_BYTES: usize = 32;
const MAX_KEYRING_FILE_BYTES: u64 = 64 * 1024;

/// 服务账号、API Key 与用户委托查询的运行时安全边界。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServiceAccountsConfig {
    pub enabled: bool,
    pub pepper_keyring_file: String,
    pub active_pepper_version: i32,
    pub max_active_credentials: u32,
    pub max_credential_days: u32,
    pub default_delegation_hours: u32,
    pub max_delegation_days: u32,
    pub default_requests_per_minute: u32,
    pub max_concurrent_queries: u32,
    pub query_timeout_ms: u64,
    pub max_page_size: u64,
    pub max_response_bytes: usize,
}

impl Default for ServiceAccountsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            pepper_keyring_file: String::new(),
            active_pepper_version: 1,
            max_active_credentials: 2,
            max_credential_days: 90,
            default_delegation_hours: 24,
            max_delegation_days: 30,
            default_requests_per_minute: 60,
            max_concurrent_queries: 4,
            query_timeout_ms: 3_000,
            max_page_size: 100,
            max_response_bytes: 1_048_576,
        }
    }
}

impl ServiceAccountsConfig {
    /// 校验不能由部署配置放宽的服务账号安全上限。
    pub fn validate(&self) -> Result<(), String> {
        if self.active_pepper_version <= 0 {
            return Err("service_accounts.active_pepper_version 必须是正整数".into());
        }
        if !(1..=2).contains(&self.max_active_credentials) {
            return Err("service_accounts.max_active_credentials 必须在 1 到 2 之间".into());
        }
        if !(1..=90).contains(&self.max_credential_days) {
            return Err("service_accounts.max_credential_days 必须在 1 到 90 之间".into());
        }
        if !(1..=30).contains(&self.max_delegation_days) {
            return Err("service_accounts.max_delegation_days 必须在 1 到 30 之间".into());
        }
        if self.default_delegation_hours == 0
            || u64::from(self.default_delegation_hours) > u64::from(self.max_delegation_days) * 24
        {
            return Err(
                "service_accounts.default_delegation_hours 必须大于 0 且不超过最大委托期限".into(),
            );
        }
        if !(1..=10_000).contains(&self.default_requests_per_minute) {
            return Err(
                "service_accounts.default_requests_per_minute 必须在 1 到 10000 之间".into(),
            );
        }
        if !(1..=64).contains(&self.max_concurrent_queries) {
            return Err("service_accounts.max_concurrent_queries 必须在 1 到 64 之间".into());
        }
        if !(100..=30_000).contains(&self.query_timeout_ms) {
            return Err("service_accounts.query_timeout_ms 必须在 100 到 30000 之间".into());
        }
        if !(1..=100).contains(&self.max_page_size) {
            return Err("service_accounts.max_page_size 必须在 1 到 100 之间".into());
        }
        if !(1_024..=1_048_576).contains(&self.max_response_bytes) {
            return Err("service_accounts.max_response_bytes 必须在 1024 到 1048576 之间".into());
        }
        if self.enabled && self.pepper_keyring_file.trim().is_empty() {
            return Err("启用 service_accounts 时 pepper_keyring_file 不能为空".into());
        }
        Ok(())
    }

    /// 从外部只读文件加载 Pepper；任何解析或安全校验失败都会阻止功能启动。
    pub fn load_pepper_keyring(&self, jwt_secret: &str) -> Result<PepperKeyring, String> {
        self.validate()?;
        if !self.enabled {
            return Err("service_accounts 未启用，不能加载 Pepper Keyring".into());
        }
        PepperKeyring::load(
            Path::new(self.pepper_keyring_file.trim()),
            self.active_pepper_version,
            jwt_secret.as_bytes(),
        )
    }
}

/// 已解码并完成安全校验的 Pepper 版本集合。
///
/// 该类型刻意不实现 `Debug`、`Serialize` 或字符串转换，避免密钥进入日志。
pub struct PepperKeyring {
    active_version: i32,
    peppers: BTreeMap<i32, Vec<u8>>,
}

impl PepperKeyring {
    fn load(path: &Path, active_version: i32, jwt_secret: &[u8]) -> Result<Self, String> {
        validate_keyring_file(path)?;
        let raw = fs::read(path)
            .map_err(|error| format!("无法读取 Pepper Keyring {}: {error}", path.display()))?;
        if u64::try_from(raw.len()).unwrap_or(u64::MAX) > MAX_KEYRING_FILE_BYTES {
            return Err(format!(
                "Pepper Keyring {} 超过 {} 字节上限",
                path.display(),
                MAX_KEYRING_FILE_BYTES
            ));
        }
        let text = std::str::from_utf8(&raw)
            .map_err(|_| format!("Pepper Keyring {} 必须使用 UTF-8 编码", path.display()))?;
        let parsed: PepperKeyringFile = match path.extension().and_then(|value| value.to_str()) {
            Some("json") => serde_json::from_str(text)
                .map_err(|error| format!("解析 Pepper Keyring JSON 失败: {error}"))?,
            Some("toml") => toml::from_str(text)
                .map_err(|error| format!("解析 Pepper Keyring TOML 失败: {error}"))?,
            _ => return Err("Pepper Keyring 文件扩展名必须是 .json 或 .toml".into()),
        };
        if parsed.peppers.is_empty() || parsed.peppers.len() > MAX_PEPPER_VERSIONS {
            return Err(format!(
                "Pepper Keyring 必须包含 1 到 {MAX_PEPPER_VERSIONS} 个版本"
            ));
        }
        let mut peppers = BTreeMap::new();
        for item in parsed.peppers {
            if item.version <= 0 {
                return Err("Pepper 版本必须是正整数".into());
            }
            let key = STANDARD
                .decode(item.key_base64.as_bytes())
                .map_err(|_| format!("Pepper 版本 {} 不是严格 Base64", item.version))?;
            if STANDARD.encode(&key) != item.key_base64 {
                return Err(format!(
                    "Pepper 版本 {} 必须使用带标准填充的规范 Base64",
                    item.version
                ));
            }
            if key.len() < MIN_PEPPER_BYTES {
                return Err(format!(
                    "Pepper 版本 {} 解码后至少需要 {MIN_PEPPER_BYTES} 字节",
                    item.version
                ));
            }
            if key.as_slice() == jwt_secret {
                return Err(format!(
                    "Pepper 版本 {} 不得复用 auth.jwt_secret",
                    item.version
                ));
            }
            if peppers.insert(item.version, key).is_some() {
                return Err(format!("Pepper 版本 {} 重复", item.version));
            }
        }
        if !peppers.contains_key(&active_version) {
            return Err(format!("活动 Pepper 版本 {active_version} 不存在"));
        }
        Ok(Self {
            active_version,
            peppers,
        })
    }

    pub const fn active_version(&self) -> i32 {
        self.active_version
    }

    pub fn active(&self) -> (i32, &[u8]) {
        (
            self.active_version,
            self.peppers
                .get(&self.active_version)
                .expect("活动 Pepper 已在加载时校验"),
        )
    }

    pub fn get(&self, version: i32) -> Option<&[u8]> {
        self.peppers.get(&version).map(Vec::as_slice)
    }

    /// 遍历全部受支持版本，供不携带版本选择器的委托令牌做常量数量候选校验。
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (i32, &[u8])> {
        self.peppers
            .iter()
            .map(|(version, key)| (*version, key.as_slice()))
    }

    pub fn len(&self) -> usize {
        self.peppers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peppers.is_empty()
    }

    /// 把已校验的密钥材料移交给组合根，避免跨边界再复制一份密钥字节。
    pub fn into_parts(self) -> (i32, BTreeMap<i32, Vec<u8>>) {
        (self.active_version, self.peppers)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PepperKeyringFile {
    peppers: Vec<PepperEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PepperEntry {
    version: i32,
    key_base64: String,
}

fn validate_keyring_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法检查 Pepper Keyring {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Pepper Keyring {} 必须是普通文件且不能是符号链接",
            path.display()
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_KEYRING_FILE_BYTES {
        return Err(format!(
            "Pepper Keyring {} 必须为非空且不超过 {} 字节",
            path.display(),
            MAX_KEYRING_FILE_BYTES
        ));
    }
    validate_unix_permissions(path, &metadata)?;
    Ok(())
}

#[cfg(unix)]
fn validate_unix_permissions(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;

    if metadata.permissions().mode() & 0o022 != 0 {
        return Err(format!(
            "Pepper Keyring {} 不能向组或其他用户开放写权限",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_unix_permissions(_path: &Path, _metadata: &fs::Metadata) -> Result<(), String> {
    Ok(())
}
