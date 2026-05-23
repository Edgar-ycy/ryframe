// ========== 模块声明 ==========
mod constants;
mod error;
mod result;

pub mod annotations;
pub mod enums;
pub mod utils;

// ========== 核心类型重导出（方便其他 crate 用 ryframe_common::AppError 直接引用）==========
pub use constants::*;
pub use error::{ApiResponse, AppError};
pub use result::AppResult;

// ========== 枚举重导出 ==========
pub use enums::business_type::BusinessType;
pub use enums::user_status::UserStatus;

// ========== 注解重导出 ==========
pub use annotations::data_scope::{DataScope, DataScopeContext};
