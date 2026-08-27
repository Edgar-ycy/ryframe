#[path = "adapter_contracts/excel_and_i18n.rs"]
mod excel_and_i18n;
#[cfg(feature = "redis")]
#[path = "adapter_contracts/redis_protocol.rs"]
mod redis_protocol;
#[path = "adapter_contracts/storage_local.rs"]
mod storage_local;
#[path = "adapter_contracts/storage_s3.rs"]
mod storage_s3;
