#![forbid(unsafe_code)]

//! RyFrame 的 SMTP 邮件发送组件。

mod email;

pub use email::{EmailConfig, EmailSender};
