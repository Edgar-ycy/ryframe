use super::frame::{QueueTextResult, queue_text, serialize_hello_frame, serialize_message_frame};
use super::*;

type MessageIdentity = (String, i64);

/// 仅管理已通过票据校验的消息连接，并以有界队列保护服务端内存。
#[derive(Clone)]
pub struct MessageHub {
    connections: Arc<DashMap<String, HubConnection>>,
    connections_by_identity: Arc<DashMap<MessageIdentity, HashSet<String>>>,
    replay_degraded_identities: Arc<DashMap<MessageIdentity, ()>>,
    replay_trigger: mpsc::Sender<MessageIdentity>,
    replay_receiver: Arc<Mutex<Option<mpsc::Receiver<MessageIdentity>>>>,
    pub(super) localizer: Arc<Localizer>,
    pub(super) config: MessagingConfig,
}

struct HubConnection {
    tenant_id: String,
    user_id: i64,
    locale: Locale,
    sender: mpsc::Sender<Message>,
    shutdown: watch::Sender<bool>,
    ready: AtomicBool,
}

impl MessageHub {
    /// 创建空的本实例连接中心。
    pub fn new(localizer: Arc<Localizer>, config: MessagingConfig) -> Self {
        let (replay_trigger, replay_receiver) = mpsc::channel(config.outbound_buffer);
        Self {
            connections: Arc::new(DashMap::new()),
            connections_by_identity: Arc::new(DashMap::new()),
            replay_degraded_identities: Arc::new(DashMap::new()),
            replay_trigger,
            replay_receiver: Arc::new(Mutex::new(Some(replay_receiver))),
            localizer,
            config,
        }
    }

    /// 返回当前实例的活动连接数量。
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// 通知所有已注册连接主动关闭，使进程优雅关闭不会被长连接无限延迟。
    pub fn shutdown_all(&self) {
        for connection in self.connections.iter() {
            let sender = connection.sender.clone();
            let shutdown = connection.shutdown.clone();
            let close = Message::Close(Some(CloseFrame {
                code: 1001,
                reason: "服务器正在关闭".into(),
            }));
            match sender.try_send(close) {
                Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    // 队列已满的连接本身就是慢消费者，沿用已有的强制关闭路径。
                    let _ = shutdown.send(true);
                }
            }
        }
    }

    pub(super) fn try_register(
        &self,
        ticket: &WebSocketTicket,
        sender: mpsc::Sender<Message>,
        shutdown: watch::Sender<bool>,
    ) -> Option<String> {
        if !self.config.enabled {
            return None;
        }
        let id = uuid::Uuid::now_v7().to_string();
        let identity = (ticket.tenant_id.clone(), ticket.user_id);
        let mut connections = self
            .connections_by_identity
            .entry(identity.clone())
            .or_default();
        if connections.len() >= self.config.max_connections_per_user {
            return None;
        }
        self.connections.insert(
            id.clone(),
            HubConnection {
                tenant_id: ticket.tenant_id.clone(),
                user_id: ticket.user_id,
                locale: Locale::parse(&ticket.locale).unwrap_or(Locale::DEFAULT),
                sender,
                shutdown,
                ready: AtomicBool::new(false),
            },
        );
        connections.insert(id.clone());
        drop(connections);
        ryframe_middleware::metrics::set_ws_connections(self.connection_count());
        Some(id)
    }

    pub(super) fn unregister(&self, connection_id: &str) {
        let Some((_, connection)) = self.connections.remove(connection_id) else {
            return;
        };
        let identity = (connection.tenant_id, connection.user_id);
        let mut identity_removed = false;
        if let Entry::Occupied(mut entry) = self.connections_by_identity.entry(identity.clone()) {
            entry.get_mut().remove(connection_id);
            if entry.get().is_empty() {
                entry.remove();
                identity_removed = true;
            }
        }
        if identity_removed {
            self.replay_degraded_identities.remove(&identity);
        }
        ryframe_middleware::metrics::set_ws_connections(self.connection_count());
    }

    fn report_replay_success(&self, identity: &MessageIdentity) {
        if self.replay_degraded_identities.remove(identity).is_some() {
            tracing::info!(
                tenant_id = %identity.0,
                user_id = identity.1,
                "消息收件箱共享补拉已恢复"
            );
        }
    }

    fn report_replay_failure(&self, identity: &MessageIdentity, error: &impl std::fmt::Display) {
        if self
            .replay_degraded_identities
            .insert(identity.clone(), ())
            .is_none()
        {
            tracing::warn!(
                %error,
                tenant_id = %identity.0,
                user_id = identity.1,
                "消息收件箱共享补拉失败，将在后续周期重试"
            );
        } else {
            tracing::debug!(
                %error,
                tenant_id = %identity.0,
                user_id = identity.1,
                "消息收件箱共享补拉仍不可用"
            );
        }
    }

    fn online_user_ids(&self) -> Vec<i64> {
        self.connections_by_identity
            .iter()
            .map(|entry| entry.key().1)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn online_identities(&self) -> BTreeSet<MessageIdentity> {
        self.connections_by_identity
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    fn request_replay(&self, identity: MessageIdentity) {
        match self.replay_trigger.try_send(identity) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::debug!("消息共享补拉触发队列已满，将由周期扫描恢复");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::warn!("消息共享补拉调度器未运行");
            }
        }
    }

    pub(super) fn initialize_connection(
        &self,
        connection_id: &str,
        ticket: &WebSocketTicket,
    ) -> bool {
        let Some(connection) = self.connections.get(connection_id) else {
            tracing::warn!(connection_id, "WebSocket 连接在初始化前已移除");
            return false;
        };
        if queue_text(
            &connection.sender,
            serialize_hello_frame(
                connection_id,
                &ticket.locale,
                self.config.replay_interval_seconds,
            ),
        ) != QueueTextResult::Queued
        {
            tracing::warn!(connection_id, "WebSocket hello 帧无法进入发送队列");
            return false;
        }
        connection.ready.store(true, Ordering::Release);
        drop(connection);
        self.request_replay((ticket.tenant_id.clone(), ticket.user_id));
        true
    }

    fn connection_ids_for_identity(&self, tenant_id: &str, user_id: i64) -> Vec<String> {
        self.connections_by_identity
            .get(&(tenant_id.to_owned(), user_id))
            .map(|connections| connections.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 将持久化消息发送给本实例中属于指定收件人的全部连接。
    pub fn send_to_user(&self, tenant_id: &str, user_id: i64, template: &MessageTemplate) -> usize {
        let mut delivered = 0;
        let mut disconnected: Vec<String> = Vec::new();

        for connection_id in self.connection_ids_for_identity(tenant_id, user_id) {
            let Some(connection) = self.connections.get(&connection_id) else {
                continue;
            };
            if !connection.ready.load(Ordering::Acquire) {
                continue;
            }
            let locale = connection.locale;
            let sender = connection.sender.clone();
            let shutdown = connection.shutdown.clone();
            drop(connection);
            let message = render_message(template, &self.localizer, locale);
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
    ) -> HttpResult<usize> {
        let online_user_ids = self.online_user_ids();
        if online_user_ids.is_empty() {
            return Ok(0);
        }
        let recipients = service
            .unacknowledged_recipients_for_online_users(message_id, &online_user_ids)
            .await?;
        let mut count = 0;
        for record in recipients {
            count += self.send_to_user(&record.tenant_id, record.user_id, &record.message);
        }
        Ok(count)
    }

    /// 在 Redis 可用时订阅跨实例唤醒信号；掉线后按指数退避重连。
    pub fn spawn_redis_listener(
        &self,
        redis: Option<RedisClient>,
        service: Arc<MessageService>,
    ) -> Option<JoinHandle<()>> {
        ryframe_middleware::metrics::set_message_redis_listener_connected(false);
        if !self.config.enabled {
            return None;
        }
        let redis = redis?;
        let hub = self.clone();
        Some(tokio::spawn(async move {
            let mut retry_seconds = 1_u64;
            let mut degraded = false;
            loop {
                match redis.subscribe(MESSAGE_DISPATCH_REDIS_CHANNEL).await {
                    Ok(subscription) => {
                        ryframe_middleware::metrics::set_message_redis_listener_connected(true);
                        if degraded {
                            tracing::info!(
                                channel = MESSAGE_DISPATCH_REDIS_CHANNEL,
                                "消息 WebSocket Redis 订阅已恢复"
                            );
                        } else {
                            tracing::info!(
                                channel = MESSAGE_DISPATCH_REDIS_CHANNEL,
                                "消息 WebSocket Redis 订阅已建立"
                            );
                        }
                        degraded = false;
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
                        ryframe_middleware::metrics::set_message_redis_listener_connected(false);
                        if !degraded {
                            tracing::warn!("消息 WebSocket Redis 订阅已中断");
                            degraded = true;
                        } else {
                            tracing::debug!("消息 WebSocket Redis 订阅仍不可用");
                        }
                    }
                    Err(error) => {
                        ryframe_middleware::metrics::set_message_redis_listener_connected(false);
                        if !degraded {
                            tracing::warn!(%error, "无法建立消息 WebSocket Redis 订阅");
                            degraded = true;
                        } else {
                            tracing::debug!(%error, "消息 WebSocket Redis 订阅仍不可用");
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(retry_seconds)).await;
                retry_seconds = retry_seconds.saturating_mul(2).min(30);
            }
        }))
    }

    /// 启动本 API 实例唯一的共享补拉调度器，按租户用户去重查询后向全部连接扇出。
    pub fn spawn_replay_scheduler(
        &self,
        service: Arc<MessageService>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Option<JoinHandle<()>> {
        if !self.config.enabled {
            return None;
        }
        let mut receiver_guard = self
            .replay_receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut replay_receiver = receiver_guard.take()?;
        drop(receiver_guard);

        let hub = self.clone();
        let interval = Duration::from_secs(self.config.replay_interval_seconds);
        let jitter = replay_startup_jitter(self.config.replay_jitter_seconds);
        Some(tokio::spawn(async move {
            if !jitter.is_zero() {
                tokio::select! {
                    _ = tokio::time::sleep(jitter) => {}
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return;
                        }
                    }
                }
            }

            let mut scan = tokio::time::interval(interval);
            scan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            scan.tick().await;
            loop {
                let identities = tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            break;
                        }
                        continue;
                    }
                    identity = replay_receiver.recv() => {
                        let Some(identity) = identity else {
                            break;
                        };
                        let mut identities = BTreeSet::from([identity]);
                        while let Ok(identity) = replay_receiver.try_recv() {
                            identities.insert(identity);
                        }
                        identities
                    }
                    _ = scan.tick() => hub.online_identities(),
                };

                replay_identities(&hub, &service, &shutdown, identities).await;
            }
            tracing::debug!("消息共享补拉调度器已停止");
        }))
    }
}

fn replay_startup_jitter(max_seconds: u64) -> Duration {
    if max_seconds == 0 {
        return Duration::ZERO;
    }
    let sample = uuid::Uuid::now_v7().as_u128() as u64;
    Duration::from_secs(sample % (max_seconds + 1))
}

async fn replay_identities(
    hub: &MessageHub,
    service: &MessageService,
    shutdown: &watch::Receiver<bool>,
    identities: BTreeSet<MessageIdentity>,
) {
    for (tenant_id, user_id) in identities {
        if *shutdown.borrow() {
            break;
        }
        let identity = (tenant_id.clone(), user_id);
        match service
            .unacknowledged_for_identity(&tenant_id, user_id, hub.config.replay_batch_size)
            .await
        {
            Ok(page) => {
                ryframe_middleware::metrics::record_message_replay_query("success");
                hub.report_replay_success(&identity);
                for record in page.records {
                    hub.send_to_user(&tenant_id, user_id, &record);
                }
            }
            Err(error) => {
                ryframe_middleware::metrics::record_message_replay_query("error");
                hub.report_replay_failure(&identity, &error);
            }
        }
    }
}
