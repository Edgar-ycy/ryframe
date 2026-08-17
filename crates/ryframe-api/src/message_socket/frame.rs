use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientFrame {
    v: u8,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    ids: Vec<String>,
}

#[derive(Clone, Copy)]
enum ClientFrameError {
    InvalidFrame,
    InvalidMessageId,
    AcknowledgeFailed,
    AcknowledgeLimit,
    UnsupportedFrame,
}

impl ClientFrameError {
    const fn code(self) -> &'static str {
        match self {
            Self::InvalidFrame => "invalid_frame",
            Self::InvalidMessageId => "invalid_message_id",
            Self::AcknowledgeFailed => "acknowledge_failed",
            Self::AcknowledgeLimit => "acknowledge_limit",
            Self::UnsupportedFrame => "unsupported_frame",
        }
    }

    const fn localization_key(self) -> &'static str {
        match self {
            Self::InvalidFrame => "message_socket.invalid_frame",
            Self::InvalidMessageId => "message_socket.invalid_message_id",
            Self::AcknowledgeFailed => "message_socket.acknowledge_failed",
            Self::AcknowledgeLimit => "message_socket.acknowledge_limit",
            Self::UnsupportedFrame => "message_socket.unsupported_frame",
        }
    }
}

fn parse_acknowledgement_ids(ids: &[String]) -> Result<Vec<i64>, ()> {
    ids.iter()
        .map(|id| id.parse::<i64>().ok().filter(|value| *value > 0).ok_or(()))
        .collect()
}

pub(super) async fn handle_client_frame(
    service: &MessageService,
    ticket: &WebSocketTicket,
    localizer: &Localizer,
    locale: Locale,
    outbound: &mpsc::Sender<Message>,
    raw: &str,
) {
    let parsed: ClientFrame = match serde_json::from_str::<ClientFrame>(raw) {
        Ok(frame) if frame.v == 1 => frame,
        _ => {
            let _ = queue_text(
                outbound,
                localized_error_frame(localizer, locale, ClientFrameError::InvalidFrame),
            );
            return;
        }
    };

    match parsed.kind.as_str() {
        "ping" => {
            let _ = queue_text(outbound, serialize_simple_frame("pong"));
        }
        "ack" if !parsed.ids.is_empty() && parsed.ids.len() <= 100 => {
            let ids = match parse_acknowledgement_ids(&parsed.ids) {
                Ok(ids) => ids,
                Err(_) => {
                    let _ = queue_text(
                        outbound,
                        localized_error_frame(
                            localizer,
                            locale,
                            ClientFrameError::InvalidMessageId,
                        ),
                    );
                    return;
                }
            };
            let started = std::time::Instant::now();
            match service
                .acknowledge_for_identity(&ticket.tenant_id, ticket.user_id, &ids)
                .await
            {
                Ok(_) => {
                    ryframe_middleware::metrics::observe_message_ack_latency(started.elapsed());
                    let _ = queue_text(outbound, serialize_ack_frame(&parsed.ids));
                }
                Err(error) => {
                    tracing::warn!(%error, user_id = ticket.user_id, "WebSocket 消息确认失败");
                    let _ = queue_text(
                        outbound,
                        localized_error_frame(
                            localizer,
                            locale,
                            ClientFrameError::AcknowledgeFailed,
                        ),
                    );
                }
            }
        }
        "ack" => {
            let _ = queue_text(
                outbound,
                localized_error_frame(localizer, locale, ClientFrameError::AcknowledgeLimit),
            );
        }
        _ => {
            let _ = queue_text(
                outbound,
                localized_error_frame(localizer, locale, ClientFrameError::UnsupportedFrame),
            );
        }
    }
}

/// 向单条连接的有界队列写入文本帧，并保留队列饱和与序列化失败的区别。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QueueTextResult {
    Queued,
    Full,
    Closed,
    SerializationFailed,
}

pub(super) fn queue_text(
    outbound: &mpsc::Sender<Message>,
    payload: HttpResult<String>,
) -> QueueTextResult {
    let Ok(payload) = payload else {
        tracing::error!("消息 WebSocket 帧序列化失败");
        return QueueTextResult::SerializationFailed;
    };
    match outbound.try_send(Message::Text(payload.into())) {
        Ok(()) => QueueTextResult::Queued,
        Err(mpsc::error::TrySendError::Full(_)) => QueueTextResult::Full,
        Err(mpsc::error::TrySendError::Closed(_)) => QueueTextResult::Closed,
    }
}

pub(super) fn serialize_hello_frame(
    connection_id: &str,
    locale: &str,
    heartbeat_secs: u64,
) -> HttpResult<String> {
    #[derive(Serialize)]
    struct Hello<'a> {
        v: u8,
        #[serde(rename = "type")]
        kind: &'static str,
        connection_id: &'a str,
        heartbeat_secs: u64,
        locale: &'a str,
    }
    serialize_frame(&Hello {
        v: 1,
        kind: "hello",
        connection_id,
        heartbeat_secs,
        locale,
    })
}

pub(super) fn serialize_message_frame(message: &MessageVo) -> HttpResult<String> {
    #[derive(Serialize)]
    struct Delivery<'a> {
        v: u8,
        #[serde(rename = "type")]
        kind: &'static str,
        message: &'a MessageVo,
    }
    serialize_frame(&Delivery {
        v: 1,
        kind: "message",
        message,
    })
}

pub(super) fn serialize_tenant_context_changed_frame(
    snapshot: &ryframe_tenant_db::TenantRuntimeSnapshot,
) -> HttpResult<String> {
    #[derive(Serialize)]
    struct TenantContextChanged<'a> {
        v: u8,
        #[serde(rename = "type")]
        kind: &'static str,
        authorization_epoch: u64,
        runtime_epoch: String,
        placement_generation: String,
        business_data_state: &'a str,
    }
    serialize_frame(&TenantContextChanged {
        v: 1,
        kind: "tenant_context_changed",
        authorization_epoch: snapshot.authorization_epoch(),
        runtime_epoch: snapshot.runtime_epoch().to_string(),
        placement_generation: snapshot.placement_generation().to_string(),
        business_data_state: snapshot.business_data_state().as_str(),
    })
}

fn serialize_ack_frame(ids: &[String]) -> HttpResult<String> {
    #[derive(Serialize)]
    struct Ack<'a> {
        v: u8,
        #[serde(rename = "type")]
        kind: &'static str,
        ids: &'a [String],
    }
    serialize_frame(&Ack {
        v: 1,
        kind: "ack",
        ids,
    })
}

fn serialize_simple_frame(kind: &'static str) -> HttpResult<String> {
    #[derive(Serialize)]
    struct Simple {
        v: u8,
        #[serde(rename = "type")]
        kind: &'static str,
    }
    serialize_frame(&Simple { v: 1, kind })
}

fn localized_error_frame(
    localizer: &Localizer,
    locale: Locale,
    error: ClientFrameError,
) -> HttpResult<String> {
    let message = localizer.translate(locale, error.localization_key());
    serialize_error_frame(error.code(), &message)
}

fn serialize_error_frame(code: &'static str, message: &str) -> HttpResult<String> {
    #[derive(Serialize)]
    struct ErrorFrame<'a> {
        v: u8,
        #[serde(rename = "type")]
        kind: &'static str,
        code: &'static str,
        message: &'a str,
    }
    serialize_frame(&ErrorFrame {
        v: 1,
        kind: "error",
        code,
        message,
    })
}

fn serialize_frame(frame: &impl Serialize) -> HttpResult<String> {
    serde_json::to_string(frame).map_err(|error| {
        HttpAppError::from(AppError::Internal(format!(
            "消息 WebSocket 帧序列化失败: {error}"
        )))
    })
}
