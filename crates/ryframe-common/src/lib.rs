// ========== 模块声明 ==========
mod constants;
mod error;
mod result;
mod sql_log_flag;

pub mod annotations;
pub mod enums;
pub mod i18n;
pub mod utils;

// ========== 核心类型重导出（方便其他 crate 用 ryframe_common::AppError 直接引用）==========
// ========== 注解重导出 ==========
pub use annotations::data_scope::{DataScope, DataScopeContext};
pub use constants::*;
// ========== 枚举重导出 ==========
pub use enums::business_type::BusinessType;
pub use enums::user_status::UserStatus;
pub use error::{ApiPageResponse, ApiResponse, AppError};
pub use result::AppResult;
pub use sql_log_flag::{enable_sql_full_log, is_sql_full_log};
