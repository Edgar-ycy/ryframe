#[path = "adapter_contracts/backup.rs"]
mod backup;
#[path = "adapter_contracts/backup_s3.rs"]
mod backup_s3;
#[path = "adapter_contracts/excel_and_i18n.rs"]
mod excel_and_i18n;
#[cfg(feature = "redis-api")]
#[path = "adapter_contracts/redis_protocol.rs"]
mod redis_protocol;
#[path = "adapter_contracts/storage_local.rs"]
mod storage_local;
#[path = "adapter_contracts/storage_s3.rs"]
mod storage_s3;
