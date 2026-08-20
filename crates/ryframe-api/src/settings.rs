use std::collections::HashMap;

use ryframe_kernel::PaginationPolicy;

pub const SINGLE_TENANT_ID: &str = "system";

#[derive(Clone, Debug)]
pub struct MultiTenancySettings {
    pub enabled: bool,
}

impl MultiTenancySettings {
    pub fn fixed_tenant_id(&self) -> Option<&'static str> {
        (!self.enabled).then_some(SINGLE_TENANT_ID)
    }

    pub fn allows_tenant(&self, tenant_id: &str) -> bool {
        self.enabled || tenant_id == SINGLE_TENANT_ID
    }
}

#[derive(Clone, Debug)]
pub struct UploadSettings {
    pub file_max_bytes: usize,
    pub avatar_max_bytes: usize,
    pub multipart_envelope_bytes: usize,
    pub upload_timeout_seconds: u64,
    pub api_timeout_seconds: u64,
}

#[derive(Clone, Debug)]
pub struct CorsSettings {
    pub allow_origins: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RateLimitSettings {
    pub enabled: bool,
    pub capacity: u32,
    pub window_secs: u64,
    pub enable_user_rate_limit: bool,
    pub user_window_secs: u64,
    pub user_capacity: u32,
    pub api_limits: HashMap<String, u32>,
    pub api_window_secs: u64,
}

#[derive(Clone, Debug)]
pub struct MessagingSettings {
    pub enabled: bool,
    pub max_connections_per_user: usize,
    pub outbound_buffer: usize,
    pub replay_interval_seconds: u64,
    pub replay_jitter_seconds: u64,
    pub replay_batch_size: u64,
}

#[derive(Clone, Debug)]
pub struct JobRuntimeSettings {
    pub mode: String,
    pub scheduler_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct StorageRuntimeSettings {
    pub backend: String,
    pub endpoint: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HttpRuntimeSettings {
    pub production: bool,
    pub telemetry_enabled: bool,
    pub api_docs_enabled: bool,
    pub pagination: PaginationPolicy,
    pub multi_tenancy: MultiTenancySettings,
    pub upload: UploadSettings,
    pub cors: CorsSettings,
    pub rate_limit: RateLimitSettings,
    pub messaging: MessagingSettings,
    pub jobs: JobRuntimeSettings,
    pub object_storage: StorageRuntimeSettings,
    pub redis_configured: bool,
    pub user_import_max_file_bytes: usize,
    pub tenant_config_max_package_bytes: usize,
}
