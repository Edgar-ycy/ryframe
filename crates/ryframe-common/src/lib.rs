// ========== 模块声明 ==========
mod actor_context;
mod constants;
mod error;
mod result;

pub mod annotations;
pub mod enums;
pub mod i18n;
pub mod utils;

// 为既有依赖 `ryframe-common` 的调用方保留领域类型入口。
pub use actor_context::ActorContext;
pub use annotations::data_scope::{DataScope, DataScopeContext};
pub use constants::*;
pub use enums::business_type::BusinessType;
pub use enums::user_status::UserStatus;
pub use error::{
    ApiEmptyResponse, ApiPageResponse, ApiResponse, AppError, HttpAppError, app_error_response,
};
pub use result::AppResult;
pub use ryframe_excel::define_excel_mapping;
pub use ryframe_kernel::{AppError as KernelAppError, AppResult as KernelAppResult, ErrorCode};
