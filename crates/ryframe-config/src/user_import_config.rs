use serde::Deserialize;

/// 异步用户导入的容量与并发限制。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserImportConfig {
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: usize,
    #[serde(default = "default_max_rows")]
    pub max_rows: usize,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_max_active_per_tenant")]
    pub max_active_per_tenant: usize,
    #[serde(default = "default_hash_parallelism")]
    pub hash_parallelism: usize,
}

impl Default for UserImportConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: default_max_file_bytes(),
            max_rows: default_max_rows(),
            batch_size: default_batch_size(),
            max_active_per_tenant: default_max_active_per_tenant(),
            hash_parallelism: default_hash_parallelism(),
        }
    }
}

impl UserImportConfig {
    /// 校验导入容量，并确保它不突破通用上传上限。
    pub fn validate(&self, upload_file_max_bytes: usize) -> Result<(), String> {
        if self.max_file_bytes == 0 || self.max_file_bytes > upload_file_max_bytes {
            return Err(
                "user_import.max_file_bytes 必须大于 0 且不超过 upload.file_max_bytes".into(),
            );
        }
        if !(1..=20_000).contains(&self.max_rows) {
            return Err("user_import.max_rows 必须在 1 到 20000 之间".into());
        }
        if !(10..=1_000).contains(&self.batch_size) || self.batch_size > self.max_rows {
            return Err("user_import.batch_size 必须在 10 到 1000 之间且不超过最大行数".into());
        }
        if !(1..=10).contains(&self.max_active_per_tenant) {
            return Err("user_import.max_active_per_tenant 必须在 1 到 10 之间".into());
        }
        if !(1..=8).contains(&self.hash_parallelism) {
            return Err("user_import.hash_parallelism 必须在 1 到 8 之间".into());
        }
        Ok(())
    }
}

const fn default_max_file_bytes() -> usize {
    10 * 1024 * 1024
}
const fn default_max_rows() -> usize {
    20_000
}
const fn default_batch_size() -> usize {
    100
}
const fn default_max_active_per_tenant() -> usize {
    1
}
const fn default_hash_parallelism() -> usize {
    2
}
