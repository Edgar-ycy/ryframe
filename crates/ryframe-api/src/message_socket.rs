use std::{
    collections::{BTreeSet, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, header},
    response::IntoResponse,
};
use dashmap::{DashMap, mapref::entry::Entry};
use futures_util::{SinkExt, StreamExt};
use ryframe_config::MessagingConfig;
use ryframe_core::RedisClient;
use ryframe_http::{HttpAppError, HttpResult};
use ryframe_i18n::{Locale, Localizer};
use ryframe_kernel::AppError;
use ryframe_service::system::{
    MESSAGE_DISPATCH_REDIS_CHANNEL, MessageService, MessageTemplate, WebSocketTicket,
};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use utoipa::ToSchema;

use crate::{
    message_presenter::{MessageVo, render_message},
    state::AppState,
};

mod frame;
mod hub;
mod session;

pub use hub::MessageHub;
pub use session::{WebSocketQuery, WebSocketTicketResponse, upgrade};
