#![forbid(unsafe_code)]

//! RyFrame 的验证码题目生成与 PNG 渲染组件。

pub use ryframe_kernel::{AppError, AppResult};

pub mod captcha;

pub use captcha::{CaptchaResult, CaptchaType, generate_captcha};
