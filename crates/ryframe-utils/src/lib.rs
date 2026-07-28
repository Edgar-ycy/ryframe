#![forbid(unsafe_code)]

//! RyFrame 的无业务依赖通用工具。

pub use ryframe_kernel::{AppError, AppResult};

pub mod data_diff;
pub mod file_upload;
pub mod ip;
pub mod key;
pub mod log_mask;
pub mod snowflake;
pub mod user_agent;

pub use data_diff::{DataDiff, DataDiffBuilder, FieldChange};
pub use log_mask::{
    is_sensitive_key, mask_bank_card, mask_by_key, mask_email, mask_id_card, mask_ip,
    mask_password, mask_phone, mask_query_string, mask_token,
};

impl From<snowflake::SnowflakeError> for AppError {
    fn from(error: snowflake::SnowflakeError) -> Self {
        tracing::error!(error = %error, "Snowflake ID 生成失败");
        Self::ServiceUnavailable("ID 生成服务暂时不可用，请稍后重试".into())
    }
}
