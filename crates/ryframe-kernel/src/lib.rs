#![forbid(unsafe_code)]

//! RyFrame 的传输无关领域核心。
//!
//! 此库只承载跨层共享的领域类型，不依赖 HTTP、数据库、缓存或具体功能实现。

mod actor_context;
pub mod constants;
mod data_scope;
pub mod enums;
mod error;
mod result;

pub use actor_context::ActorContext;
pub use constants::*;
pub use data_scope::{DataScope, DataScopeContext};
pub use enums::{BusinessType, UserStatus};
pub use error::{AppError, ErrorCode};
pub use result::AppResult;
