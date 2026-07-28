use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyConfig {
    /// 仅当套接字对端属于这些精确的 IP/CIDR 范围之一时，才解析转发请求头。
    #[serde(default)]
    pub trusted_cidrs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadLimitsConfig {
    #[serde(default = "default_file_max_bytes")]
    pub file_max_bytes: usize,
    #[serde(default = "default_avatar_max_bytes")]
    pub avatar_max_bytes: usize,
    #[serde(default = "default_multipart_envelope_bytes")]
    pub multipart_envelope_bytes: usize,
    #[serde(default = "default_upload_timeout_seconds")]
    pub upload_timeout_seconds: u64,
    #[serde(default = "default_api_timeout_seconds")]
    pub api_timeout_seconds: u64,
}

impl Default for UploadLimitsConfig {
    fn default() -> Self {
        Self {
            file_max_bytes: default_file_max_bytes(),
            avatar_max_bytes: default_avatar_max_bytes(),
            multipart_envelope_bytes: default_multipart_envelope_bytes(),
            upload_timeout_seconds: default_upload_timeout_seconds(),
            api_timeout_seconds: default_api_timeout_seconds(),
        }
    }
}

const fn default_file_max_bytes() -> usize {
    10 * 1024 * 1024
}

const fn default_avatar_max_bytes() -> usize {
    5 * 1024 * 1024
}

const fn default_multipart_envelope_bytes() -> usize {
    64 * 1024
}

const fn default_upload_timeout_seconds() -> u64 {
    120
}

const fn default_api_timeout_seconds() -> u64 {
    30
}
