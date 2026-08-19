#![forbid(unsafe_code)]

//! RyFrame 的传输无关领域核心。
//!
//! 此库只承载跨层共享的领域类型，不依赖 HTTP、数据库、缓存或具体功能实现。

mod actor_context;
pub mod constants;
mod data_scope;
pub mod enums;
mod error;
mod ip_cidr;
mod localization;
mod result;
mod snowflake_worker;

pub use actor_context::ActorContext;
pub use constants::*;
pub use data_scope::{DataScope, DataScopeContext};
pub use enums::{BusinessType, UserStatus};
pub use error::{AppError, ErrorCode};
pub use ip_cidr::IpCidr;
pub use localization::{Locale, LocalizationError, LocalizedText, Localizer};
pub use result::AppResult;
pub use snowflake_worker::{MAX_SNOWFLAKE_WORKER_ID, SnowflakeWorkerId};
