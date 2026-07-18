//! WebSocket 实时推送
//!
//! 提供：
//! - WebSocket 连接管理（连接池、心跳）
//! - 房间/频道支持（群组广播）
//! - 单用户推送
//! - 全量广播
//! - 自动断线清理
//!
//! # 使用示例
//!
//! ```
//! # #[tokio::main]
//! # async fn main() {
//! use ryframe_middleware::websocket::{WsManager, WsMessage};
//! use std::sync::Arc;
//!
//! let ws_manager = Arc::new(WsManager::new());
//! assert_eq!(ws_manager.connection_count(), 0);
//!
//! // 消息类型构造（自包含，无需连接）
//! let msg = WsMessage::text("你好");
//! assert_eq!(msg.msg_type, "text");
//!
//! let notification = WsMessage::notification("系统通知");
//! assert_eq!(notification.msg_type, "notification");
//!
//! let system_msg = WsMessage::system("服务重启");
//! assert!(system_msg.to_json().contains("服务重启"));
//! # }
//! ```

use std::{collections::HashSet, sync::Arc};

use axum::{
    extract::{
        Query, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing::{info, warn};

// ============ 类型定义 ============

/// WebSocket 连接 ID
pub type ConnectionId = String;

/// WebSocket 消息类型
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WsMessage {
    /// 消息类型（如 "notification", "chat", "system"）
    #[serde(rename = "type")]
    pub msg_type: String,
    /// 消息内容（JSON 字符串）
    pub content: String,
    /// 发送者 ID（系统消息为空）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// 时间戳
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
}

impl WsMessage {
    /// 创建文本消息
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            msg_type: "text".into(),
            content: content.into(),
            sender: None,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
        }
    }

    /// 创建通知消息
    pub fn notification(content: impl Into<String>) -> Self {
        Self {
            msg_type: "notification".into(),
            content: content.into(),
            sender: None,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
        }
    }

    /// 创建系统消息
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            msg_type: "system".into(),
            content: content.into(),
            sender: None,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
        }
    }

    /// 序列化为 JSON 字符串
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"type":"error","content":"序列化失败"}"#.into())
    }
}

/// WebSocket 升级查询参数
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WsQuery {
    /// 要加入的房间（逗号分隔）
    #[serde(default)]
    pub rooms: Option<String>,
}

// ============ 内部连接信息 ============

struct Connection {
    sender: UnboundedSender<Message>,
    rooms: HashSet<String>,
}

// ============ WebSocket 连接管理器 ============

/// WebSocket 连接管理器
///
/// 线程安全，可克隆（内部使用 `Arc`）。
/// 管理所有活跃的 WebSocket 连接，支持房间和单点推送。
#[derive(Clone)]
pub struct WsManager {
    connections: Arc<DashMap<ConnectionId, Connection>>,
    rooms: Arc<DashMap<String, HashSet<ConnectionId>>>,
}

impl WsManager {
    /// 创建新的连接管理器
    pub fn new() -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            rooms: Arc::new(DashMap::new()),
        }
    }

    /// 注册新连接
    pub fn register(&self, id: ConnectionId, sender: UnboundedSender<Message>) {
        self.connections.insert(
            id,
            Connection {
                sender,
                rooms: HashSet::new(),
            },
        );
    }

    /// 移除连接
    pub fn unregister(&self, id: &ConnectionId) {
        // 从所有房间移除
        if let Some((_, conn)) = self.connections.remove(id) {
            for room in &conn.rooms {
                if let Some(mut members) = self.rooms.get_mut(room) {
                    members.remove(id);
                    if members.is_empty() {
                        drop(members);
                        self.rooms.remove(room);
                    }
                }
            }
        }
    }

    /// 加入房间
    pub fn join_room(&self, id: &ConnectionId, room: &str) {
        if let Some(mut conn) = self.connections.get_mut(id) {
            conn.rooms.insert(room.to_string());
        }
        self.rooms
            .entry(room.to_string())
            .or_default()
            .insert(id.clone());
    }

    /// 离开房间
    pub fn leave_room(&self, id: &ConnectionId, room: &str) {
        if let Some(mut conn) = self.connections.get_mut(id) {
            conn.rooms.remove(room);
        }
        if let Some(mut members) = self.rooms.get_mut(room) {
            members.remove(id);
            if members.is_empty() {
                drop(members);
                self.rooms.remove(room);
            }
        }
    }

    /// 获取在线连接数
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// 获取房间成员数
    pub fn room_member_count(&self, room: &str) -> usize {
        self.rooms.get(room).map(|m| m.len()).unwrap_or(0)
    }

    // ============ 发送消息 ============

    /// 广播文本消息给所有连接
    pub async fn broadcast_text(&self, content: impl Into<String>) -> usize {
        let msg = WsMessage::text(content);
        self.broadcast(&msg).await
    }

    /// 广播通知消息给所有连接
    pub async fn broadcast_notification(&self, content: impl Into<String>) -> usize {
        let msg = WsMessage::notification(content);
        self.broadcast(&msg).await
    }

    /// 广播消息给所有连接
    pub async fn broadcast(&self, msg: &WsMessage) -> usize {
        let text = msg.to_json();
        let ws_msg = Message::Text(text.into());
        let mut count = 0usize;

        for entry in self.connections.iter() {
            if entry.value().sender.send(ws_msg.clone()).is_ok() {
                count += 1;
            }
        }

        count
    }

    /// 发送文本消息到指定房间
    pub async fn send_to_room_text(&self, room: &str, content: impl Into<String>) -> usize {
        let msg = WsMessage::text(content);
        self.send_to_room(room, &msg).await
    }

    /// 发送消息到指定房间
    pub async fn send_to_room(&self, room: &str, msg: &WsMessage) -> usize {
        let text = msg.to_json();
        let ws_msg = Message::Text(text.into());
        let mut count = 0usize;

        if let Some(members) = self.rooms.get(room) {
            for id in members.value() {
                if let Some(conn) = self.connections.get(id)
                    && conn.sender.send(ws_msg.clone()).is_ok()
                {
                    count += 1;
                }
            }
        }

        count
    }

    /// 发送文本消息到指定连接
    pub async fn send_to_connection_text(
        &self,
        id: &ConnectionId,
        content: impl Into<String>,
    ) -> bool {
        let msg = WsMessage::text(content);
        self.send_to_connection(id, &msg).await
    }

    /// 发送消息到指定连接
    pub async fn send_to_connection(&self, id: &ConnectionId, msg: &WsMessage) -> bool {
        if let Some(conn) = self.connections.get(id) {
            let text = msg.to_json();
            conn.sender.send(Message::Text(text.into())).is_ok()
        } else {
            false
        }
    }
}

impl Default for WsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for WsManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsManager")
            .field("connections", &self.connections.len())
            .field("rooms", &self.rooms.len())
            .finish()
    }
}

// ============ WebSocket 升级 Handler ============

/// WebSocket 升级处理函数
///
/// 可作为 axum handler 直接注册到 Router。
/// 支持 `?rooms=room1,room2` 查询参数；认证信息不得放入 URL。
///
/// # 示例
/// ```
/// # use ryframe_middleware::websocket::ws_upgrade;
/// // 可作为 axum handler 注册：
/// // Router::new()
/// //     .route("/ws", get(ws_upgrade))
/// //     .with_state(ws_manager);
/// ```
pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    axum::extract::State(manager): axum::extract::State<Arc<WsManager>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, manager, query))
}

/// 处理单个 WebSocket 连接
async fn handle_socket(socket: WebSocket, manager: Arc<WsManager>, query: WsQuery) {
    let conn_id = uuid::Uuid::new_v4().to_string();
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // 创建内部通道
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // 注册连接
    manager.register(conn_id.clone(), tx);

    // 加入指定房间
    if let Some(ref rooms) = query.rooms {
        for room in rooms.split(',').map(|s| s.trim()) {
            if !room.is_empty() {
                manager.join_room(&conn_id, room);
            }
        }
    }

    info!(
        conn_id = %conn_id,
        "WebSocket 客户端已连接 (在线: {})",
        manager.connection_count()
    );

    // 发送欢迎消息
    let welcome = WsMessage::system(format!("已连接 (ID: {})", &conn_id[..8]));
    let _ = ws_sender
        .send(Message::Text(welcome.to_json().into()))
        .await;

    // 双向消息循环
    let conn_id_clone = conn_id.clone();
    let manager_clone = manager.clone();

    // 发送任务：从内部通道读取 → 写入 WebSocket
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // 接收任务：从 WebSocket 读取 → 日志（可扩展为消息路由）
    let mut recv_task = tokio::spawn(async move {
        while let Some(result) = ws_receiver.next().await {
            match result {
                Ok(Message::Text(text)) => {
                    tracing::debug!(
                        conn_id = %conn_id_clone,
                        message_bytes = text.len(),
                        "收到 WebSocket 文本消息"
                    );
                }
                Ok(Message::Ping(_data)) => {
                    // Pong 由 axum 自动处理
                    tracing::trace!(conn_id = %conn_id_clone, "收到 Ping");
                }
                Ok(Message::Close(_)) => {
                    tracing::info!(conn_id = %conn_id_clone, "客户端主动关闭");
                    break;
                }
                Ok(Message::Pong(_)) => {
                    // 心跳响应
                }
                Ok(Message::Binary(_)) => {
                    // 暂不处理二进制
                }
                Err(e) => {
                    warn!(conn_id = %conn_id_clone, error = %e, "WebSocket 错误");
                    break;
                }
            }
        }
    });

    // 等待任一任务结束（使用 AbortHandle 避免 move 冲突）
    let send_abort = send_task.abort_handle();
    let recv_abort = recv_task.abort_handle();

    tokio::select! {
        _ = &mut send_task => {
            recv_abort.abort();
        }
        _ = &mut recv_task => {
            send_abort.abort();
        }
    }

    // 清理连接
    manager_clone.unregister(&conn_id);
    info!(
        conn_id = %conn_id,
        "WebSocket 客户端已断开 (在线: {})",
        manager_clone.connection_count()
    );
}
