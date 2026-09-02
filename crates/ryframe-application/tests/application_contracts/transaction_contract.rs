use std::sync::Arc;

#[cfg(feature = "test-support")]
use chrono::Utc;
use ryframe_application::{
    AuthorizationCache, MessagingPolicy,
    ports::system::*,
    system::{content::*, identity::*, operations::*},
};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_kernel::*;

#[path = "../transaction_contract/config.rs"]
mod config;
#[path = "../transaction_contract/login_info.rs"]
mod login_info;
#[cfg(feature = "test-support")]
#[path = "../transaction_contract/notice.rs"]
mod notice;
#[path = "../transaction_contract/oper_log.rs"]
mod oper_log;
#[cfg(feature = "test-support")]
#[path = "../transaction_contract/post.rs"]
mod post;
#[path = "../transaction_contract/transaction_completion.rs"]
mod transaction_completion;
#[path = "../transaction_contract/websocket_ticket.rs"]
mod websocket_ticket;
