#![forbid(unsafe_code)]

//! RyFrame 的传输无关领域核心。
//!
//! 此库只承载跨层共享的领域类型，不依赖 HTTP、数据库、缓存或具体功能实现。

mod actor_context;
pub mod constants;
mod data_scope;
mod decimal;
pub mod enums;
mod error;
mod export_selection;
mod ip_cidr;
mod localization;
mod pagination;
mod result;
pub mod snowflake;
mod snowflake_worker;
mod tenant_id;

pub use actor_context::ActorContext;
pub use constants::*;
pub use data_scope::{DATA_SCOPE_PROJECTION_OUTBOX_EVENT_TYPE, DataScope, DataScopeContext};
pub use decimal::decimal_fits_precision;
pub use enums::{BusinessType, UserStatus};
pub use error::{AppError, ErrorCode};
pub use export_selection::{ExportCursorWindow, ExportQuerySnapshot};
pub use ip_cidr::IpCidr;
pub use localization::{Locale, LocalizationError, LocalizedText, Localizer};
pub use pagination::{PageResult, PaginationPolicy, ValidatedPageQuery};
pub use result::AppResult;
pub use snowflake_worker::{MAX_SNOWFLAKE_WORKER_ID, SnowflakeWorkerId};
pub use tenant_id::TenantId;
