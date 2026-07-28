//! 旧公共库的 HTTP 类型兼容出口。
//!
//! 新代码应直接从 `ryframe-http` 导入；本模块只为既有下游调用保留路径兼容。

pub use ryframe_http::{
    ApiEmptyResponse, ApiPageResponse, ApiResponse, AppError, HttpAppError, app_error_response,
};
