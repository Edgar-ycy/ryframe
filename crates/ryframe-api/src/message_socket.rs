use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
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
use ryframe_core::RedisClient;
use ryframe_http::{AppError, AppResult};
use ryframe_i18n::{Locale, Localizer};
use ryframe_kernel::AppError as KernelAppError;
use ryframe_service::system::{
    MESSAGE_DISPATCH_REDIS_CHANNEL, MessageDelivery, MessageService, WebSocketTicket,
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

const CONNECTION_QUEUE_CAPACITY: usize = 256;
const RESYNC_INTERVAL: Duration = Duration::from_secs(15);
const RESYNC_BATCH_SIZE: u64 = 100;

/// 申请一次性 WebSocket 票据后的 HTTP 响应。
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WebSocketTicketResponse {
    pub ticket: String,
    pub expires_in: u64,
}

/// 仅管理已通过票据校验的消息连接，并以有界队列保护服务端内存。
#[derive(Clone)]
pub struct MessageHub {
    connections: Arc<DashMap<String, HubConnection>>,
    connections_by_identity: Arc<DashMap<(String, i64), HashSet<String>>>,
    localizer: Arc<Localizer>,
}

struct HubConnection {
    tenant_id: String,
    user_id: i64,
    locale: Locale,
    sender: mpsc::Sender<Message>,
    shutdown: watch::Sender<bool>,
}

impl MessageHub {
    /// 创建空的本实例连接中心。
    pub fn new(localizer: Arc<Localizer>) -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            connections_by_identity: Arc::new(DashMap::new()),
            localizer,
        }
    }

    /// 返回当前实例的活动连接数量。
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    fn register(
        &self,
        ticket: &WebSocketTicket,
        sender: mpsc::Sender<Message>,
        shutdown: watch::Sender<bool>,
    ) -> String {
        let id = uuid::Uuid::now_v7().to_string();
        self.connections.insert(
            id.clone(),
            HubConnection {
                tenant_id: ticket.tenant_id.clone(),
                user_id: ticket.user_id,
                locale: Locale::parse(&ticket.locale).unwrap_or(Locale::DEFAULT),
                sender,
                shutdown,
            },
        );
        self.connections_by_identity
            .entry((ticket.tenant_id.clone(), ticket.user_id))
            .or_default()
            .insert(id.clone());
        ryframe_middleware::metrics::set_ws_connections(self.connection_count());
        id
    }

    fn unregister(&self, connection_id: &str) {
        let Some((_, connection)) = self.connections.remove(connection_id) else {
            return;
        };
        let identity = (connection.tenant_id, connection.user_id);
        if let Entry::Occupied(mut entry) = self.connections_by_identity.entry(identity) {
            entry.get_mut().remove(connection_id);
            if entry.get().is_empty() {
                entry.remove();
            }
        }
        ryframe_middleware::metrics::set_ws_connections(self.connection_count());
    }

    fn online_user_ids(&self) -> Vec<i64> {
        self.connections_by_identity
            .iter()
            .map(|entry| entry.key().1)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn connection_ids_for_identity(&self, tenant_id: &str, user_id: i64) -> Vec<String> {
        self.connections_by_identity
            .get(&(tenant_id.to_owned(), user_id))
            .map(|connections| connections.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 将持久化消息发送给本实例中属于指定收件人的全部连接。
    pub fn send_to_user(&self, tenant_id: &str, user_id: i64, record: &MessageDelivery) -> usize {
        let mut delivered = 0;
        let mut disconnected: Vec<String> = Vec::new();

        for connection_id in self.connection_ids_for_identity(tenant_id, user_id) {
            let Some(connection) = self.connections.get(&connection_id) else {
                continue;
            };
            let locale = connection.locale;
            let sender = connection.sender.clone();
            let shutdown = connection.shutdown.clone();
            drop(connection);
            let message = render_message(&record.message, &self.localizer, locale);
            let Ok(payload) = serialize_message_frame(&message) else {
                tracing::error!(message_id = %message.id, "消息 WebSocket 帧序列化失败");
                continue;
            };
            match sender.try_send(Message::Text(payload.into())) {
                Ok(()) => {
                    delivered += 1;
                    ryframe_middleware::metrics::record_message_delivery("delivered");
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    let _ = shutdown.send(true);
                    disconnected.push(connection_id.clone());
                    ryframe_middleware::metrics::record_message_delivery("slow_consumer");
                    tracing::warn!(connection_id, "WebSocket 慢消费者已关闭");
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    disconnected.push(connection_id);
                    ryframe_middleware::metrics::record_message_delivery("closed");
                }
            }
        }
        for id in disconnected {
            self.unregister(&id);
        }
        delivered
    }

    /// 处理 Redis 唤醒后的一条消息；收件人和正文始终从 MySQL 快照读取。
    pub async fn deliver_message(
        &self,
        service: &MessageService,
        message_id: i64,
    ) -> AppResult<usize> {
        let online_user_ids = self.online_user_ids();
        if online_user_ids.is_empty() {
            return Ok(0);
        }
        let recipients = service
            .unacknowledged_recipients_for_online_users(message_id, &online_user_ids)
            .await?;
        let mut count = 0;
        for record in recipients {
            count += self.send_to_user(&record.tenant_id, record.user_id, &record);
        }
        Ok(count)
    }

    /// 在 Redis 可用时订阅跨实例唤醒信号；掉线后按指数退避重连。
    pub fn spawn_redis_listener(
        &self,
        redis: Option<RedisClient>,
        service: Arc<MessageService>,
    ) -> Option<JoinHandle<()>> {
        let redis = redis?;
        let hub = self.clone();
        Some(tokio::spawn(async move {
            let mut retry_seconds = 1_u64;
            loop {
                match redis.subscribe(MESSAGE_DISPATCH_REDIS_CHANNEL).await {
                    Ok(subscription) => {
                        tracing::info!(
                            channel = MESSAGE_DISPATCH_REDIS_CHANNEL,
                            "消息 WebSocket Redis 订阅已建立"
                        );
                        retry_seconds = 1;
                        let mut messages = subscription.into_on_message();
                        while let Some(raw) = messages.next().await {
                            let Ok(payload) = raw.get_payload::<String>() else {
                                tracing::warn!("收到无法解析的消息唤醒负载");
                                continue;
                            };
                            let Ok(message_id) = payload.parse::<i64>() else {
                                tracing::warn!("收到无效的消息唤醒标识");
                                continue;
                            };
                            if let Err(error) = hub.deliver_message(&service, message_id).await {
                                tracing::warn!(%error, message_id, "消息在线投递失败，将由收件箱补拉恢复");
                            }
                        }
                        tracing::warn!("消息 WebSocket Redis 订阅已中断");
                    }
                    Err(error) => tracing::warn!(%error, "无法建立消息 WebSocket Redis 订阅"),
                }
                tokio::time::sleep(Duration::from_secs(retry_seconds)).await;
                retry_seconds = retry_seconds.saturating_mul(2).min(30);
            }
        }))
    }
}

/// WebSocket 升级查询参数；原始票据只会在此处使用，日志不会记录查询字符串。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSocketQuery {
    pub ticket: String,
}

/// 执行 Origin 校验、消费一次性票据并升级为消息连接。
pub async fn upgrade(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    Query(query): Query<WebSocketQuery>,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    validate_websocket_origin(&state, &headers)?;
    let ticket = match state.services.websocket_ticket.consume(&query.ticket).await {
        Ok(ticket) => {
            ryframe_middleware::metrics::record_ws_ticket("consumed");
            ticket
        }
        Err(error) => {
            let result = if matches!(error, KernelAppError::ServiceUnavailable(_)) {
                "backend_error"
            } else {
                "rejected"
            };
            ryframe_middleware::metrics::record_ws_ticket(result);
            return Err(error.into());
        }
    };
    state
        .services
        .auth
        .validate_websocket_session(
            &ticket.tenant_id,
            ticket.user_id,
            &ticket.session_id,
            ticket.user_auth_version,
            ticket.tenant_session_version,
        )
        .await?;
    let hub = state.message_hub.clone();
    let service = state.services.message.clone();
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, hub, service, ticket)))
}

async fn handle_socket(
    socket: WebSocket,
    hub: Arc<MessageHub>,
    service: Arc<MessageService>,
    ticket: WebSocketTicket,
) {
    let (outbound, mut outbound_receiver) = mpsc::channel(CONNECTION_QUEUE_CAPACITY);
    let (shutdown_sender, mut shutdown_receiver) = watch::channel(false);
    let connection_id = hub.register(&ticket, outbound.clone(), shutdown_sender.clone());
    let (mut socket_sender, mut socket_receiver) = socket.split();

    let _ = queue_text(
        &outbound,
        serialize_hello_frame(&connection_id, &ticket.locale),
    );
    replay_unacknowledged(&hub, &service, &ticket, &outbound, &shutdown_sender).await;

    let mut send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                changed = shutdown_receiver.changed() => {
                    let should_close = changed.is_ok() && *shutdown_receiver.borrow();
                    if should_close {
                        let _ = socket_sender.send(Message::Close(Some(CloseFrame {
                            code: 1013,
                            reason: "客户端消费速度过慢".into(),
                        }))).await;
                        break;
                    }
                }
                message = outbound_receiver.recv() => match message {
                    Some(message) => {
                        if socket_sender.send(message).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    });

    let outbound_for_receive = outbound.clone();
    let hub_for_receive = hub.clone();
    let shutdown_for_receive = shutdown_sender.clone();
    let localizer_for_receive = hub.localizer.clone();
    let locale_for_receive = Locale::parse(&ticket.locale).unwrap_or(Locale::DEFAULT);
    let mut receive_task = tokio::spawn(async move {
        let mut resync = tokio::time::interval(RESYNC_INTERVAL);
        resync.tick().await;
        loop {
            tokio::select! {
                frame = socket_receiver.next() => match frame {
                    Some(Ok(Message::Text(text))) => {
                        handle_client_frame(
                            &service,
                            &ticket,
                            &localizer_for_receive,
                            locale_for_receive,
                            &outbound_for_receive,
                            text.as_str(),
                        ).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) | Some(Ok(Message::Binary(_))) => {}
                    Some(Err(error)) => {
                        tracing::debug!(%error, "消息 WebSocket 接收失败");
                        break;
                    }
                },
                _ = resync.tick() => replay_unacknowledged(
                    &hub_for_receive,
                    &service,
                    &ticket,
                    &outbound_for_receive,
                    &shutdown_for_receive,
                ).await,
            }
        }
    });

    let send_abort = send_task.abort_handle();
    let receive_abort = receive_task.abort_handle();
    tokio::select! {
        _ = &mut send_task => receive_abort.abort(),
        _ = &mut receive_task => send_abort.abort(),
    }
    hub.unregister(&connection_id);
    tracing::debug!(connection_id, "消息 WebSocket 连接已关闭");
}

async fn replay_unacknowledged(
    hub: &MessageHub,
    service: &MessageService,
    ticket: &WebSocketTicket,
    outbound: &mpsc::Sender<Message>,
    shutdown: &watch::Sender<bool>,
) {
    match service
        .unacknowledged_for_identity(&ticket.tenant_id, ticket.user_id, RESYNC_BATCH_SIZE)
        .await
    {
        Ok(page) => {
            let locale = Locale::parse(&ticket.locale).unwrap_or(Locale::DEFAULT);
            for message in page.records {
                let message = render_message(&message, &hub.localizer, locale);
                match queue_text(outbound, serialize_message_frame(&message)) {
                    QueueTextResult::Queued => {}
                    QueueTextResult::Full => {
                        let _ = shutdown.send(true);
                        tracing::warn!(
                            user_id = ticket.user_id,
                            "WebSocket 补拉遇到慢消费者，已关闭连接"
                        );
                        break;
                    }
                    QueueTextResult::Closed | QueueTextResult::SerializationFailed => break,
                }
            }
        }
        Err(error) => {
            tracing::warn!(%error, user_id = ticket.user_id, "消息收件箱补拉失败");
        }
    }
}

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

async fn handle_client_frame(
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
enum QueueTextResult {
    Queued,
    Full,
    Closed,
    SerializationFailed,
}

fn queue_text(
    outbound: &mpsc::Sender<Message>,
    payload: Result<String, AppError>,
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

fn serialize_hello_frame(connection_id: &str, locale: &str) -> AppResult<String> {
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
        heartbeat_secs: RESYNC_INTERVAL.as_secs(),
        locale,
    })
}

fn serialize_message_frame(message: &MessageVo) -> AppResult<String> {
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

fn serialize_ack_frame(ids: &[String]) -> AppResult<String> {
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

fn serialize_simple_frame(kind: &'static str) -> AppResult<String> {
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
) -> AppResult<String> {
    let message = localizer.translate(locale, error.localization_key());
    serialize_error_frame(error.code(), &message)
}

fn serialize_error_frame(code: &'static str, message: &str) -> AppResult<String> {
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

fn serialize_frame(frame: &impl Serialize) -> AppResult<String> {
    serde_json::to_string(frame)
        .map_err(|error| AppError::Internal(format!("消息 WebSocket 帧序列化失败: {error}")))
}

fn validate_websocket_origin(state: &AppState, headers: &HeaderMap) -> AppResult<()> {
    match headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        Some(origin)
            if state
                .config
                .cors
                .allow_origins
                .iter()
                .any(|allowed| allowed == origin) =>
        {
            Ok(())
        }
        Some(_) => Err(AppError::Authorization("WebSocket Origin 未获允许".into())),
        None if production_environment() => Err(AppError::Authorization(
            "生产环境的 WebSocket 请求必须携带 Origin".into(),
        )),
        None => Ok(()),
    }
}

fn production_environment() -> bool {
    std::env::var("APP_ENV").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "prod" | "production"
        )
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ryframe_i18n::{Locale, Localizer};
    use ryframe_service::system::WebSocketTicket;
    use tokio::sync::{mpsc, watch};

    use super::{
        ClientFrameError, MessageHub, QueueTextResult, localized_error_frame,
        parse_acknowledgement_ids, queue_text,
    };

    fn ticket(tenant_id: &str, user_id: i64) -> WebSocketTicket {
        WebSocketTicket {
            tenant_id: tenant_id.into(),
            user_id,
            session_id: "session".into(),
            user_auth_version: 1,
            tenant_session_version: 1,
            locale: "zh-CN".into(),
        }
    }

    fn register(hub: &MessageHub, ticket: &WebSocketTicket) -> String {
        let (sender, _) = mpsc::channel(1);
        let (shutdown, _) = watch::channel(false);
        hub.register(ticket, sender, shutdown)
    }

    #[test]
    fn connection_index_tracks_identity_lifecycle() {
        let hub = MessageHub::new(Arc::new(Localizer::embedded().expect("内嵌国际化资源")));
        let tenant_a_user = ticket("tenant-a", 1);
        let tenant_b_user = ticket("tenant-b", 2);
        let first = register(&hub, &tenant_a_user);
        let second = register(&hub, &tenant_a_user);
        let third = register(&hub, &tenant_b_user);

        assert_eq!(hub.online_user_ids(), vec![1, 2]);
        assert_eq!(hub.connection_ids_for_identity("tenant-a", 1).len(), 2);
        assert_eq!(hub.connection_ids_for_identity("tenant-b", 2).len(), 1);

        hub.unregister(&first);
        assert_eq!(hub.connection_ids_for_identity("tenant-a", 1).len(), 1);
        hub.unregister(&second);
        assert!(hub.connection_ids_for_identity("tenant-a", 1).is_empty());
        hub.unregister(&third);
        assert!(hub.online_user_ids().is_empty());
    }

    #[test]
    fn queue_text_distinguishes_a_slow_consumer() {
        let (sender, _receiver) = mpsc::channel(1);

        assert_eq!(
            queue_text(&sender, Ok("first".into())),
            QueueTextResult::Queued
        );
        assert_eq!(
            queue_text(&sender, Ok("second".into())),
            QueueTextResult::Full
        );
    }

    #[test]
    fn localized_error_frames_preserve_stable_protocol_codes() {
        let localizer = Localizer::embedded().expect("内嵌国际化资源");
        let errors = [
            ClientFrameError::InvalidFrame,
            ClientFrameError::InvalidMessageId,
            ClientFrameError::AcknowledgeFailed,
            ClientFrameError::AcknowledgeLimit,
            ClientFrameError::UnsupportedFrame,
        ];

        for error in errors {
            let payload =
                localized_error_frame(&localizer, Locale::EnUs, error).expect("错误帧应可序列化");
            let frame: serde_json::Value =
                serde_json::from_str(&payload).expect("错误帧应为 JSON 对象");

            assert_eq!(frame["type"], "error");
            assert_eq!(frame["code"], error.code());
            assert_eq!(
                frame["message"],
                localizer.translate(Locale::EnUs, error.localization_key())
            );
        }
    }

    #[test]
    fn acknowledgement_ids_must_be_positive_i64_values() {
        let valid = vec!["1".into(), "42".into()];
        assert_eq!(parse_acknowledgement_ids(&valid), Ok(vec![1, 42]));

        for invalid in ["0", "-1", "not-a-number", "9223372036854775808"] {
            assert!(parse_acknowledgement_ids(&[invalid.into()]).is_err());
        }
    }
}
