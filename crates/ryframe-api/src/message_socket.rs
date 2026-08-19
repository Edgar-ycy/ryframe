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
use ryframe_adapters::RedisClient;
use ryframe_application::system::{
    MESSAGE_DISPATCH_REDIS_CHANNEL, MessageService, MessageTemplate, WebSocketTicket,
};
use ryframe_application::{AUTHORIZATION_CHANGED_REDIS_CHANNEL, AuthorizationChangedEvent};
use ryframe_config::MessagingConfig;
use ryframe_http::{HttpAppError, HttpResult};
use ryframe_kernel::{AppError, Locale, Localizer};
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
