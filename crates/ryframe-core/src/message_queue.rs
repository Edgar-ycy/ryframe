//! 统一消息队列抽象层
//!
//! 提供 MessageQueue trait 抽象，支持多种后端实现：
//! - **Noop**：空实现，用于禁用消息队列的场景
//! - **InMemory**：内存实现，用于开发/测试
//! - **Kafka**：生产级实现，需要启用 `kafka` feature
//!
//! # 使用示例
//!
//! ```
//! use ryframe_core::message_queue::{MessageQueue, InMemoryMessageQueue};
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mq = InMemoryMessageQueue::new();
//! // subscribe first, then publish (broadcast requires active receiver)
//! mq.subscribe("user.created", |msg| async move {
//!     assert_eq!(msg, b"{\"user_id\": 1}");
//!     Ok(())
//! }).await?;
//! mq.publish("user.created", b"{\"user_id\": 1}").await?;
//! # Ok(())
//! # }
//! ```

use async_trait::async_trait;
use serde::Serialize;
use std::collections::HashMap;

use tokio::sync::{RwLock, broadcast};

// ============ 核心 Trait ============

/// 消息队列错误
#[derive(Debug, thiserror::Error)]
pub enum MqError {
    #[error("消息发布失败: {0}")]
    PublishFailed(String),

    #[error("消息订阅失败: {0}")]
    SubscribeFailed(String),

    #[error("序列化失败: {0}")]
    SerializeFailed(String),

    #[error("反序列化失败: {0}")]
    DeserializeFailed(String),
}

/// 消息队列抽象
///
/// 支持发布/订阅模式，基于 topic 路由。
/// 所有实现需保证 **至少一次 (at-least-once)** 投递语义。
///
/// 注意：因 trait 需要对象安全（用于 `Arc<dyn MessageQueue>`），
/// 泛型辅助方法 `publish_json` 作为独立函数提供。
#[async_trait]
pub trait MessageQueue: Send + Sync {
    /// 发布消息到指定 topic
    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), MqError>;

    /// 订阅 topic（异步回调）
    async fn subscribe<F, Fut>(&self, topic: &str, handler: F) -> Result<(), MqError>
    where
        F: Fn(Vec<u8>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), MqError>> + Send + 'static;

    /// 检查连接状态
    async fn health_check(&self) -> bool;
}

/// 发布 JSON 序列化消息的辅助函数
///
/// 使用 `&impl MessageQueue` 避免 trait 对象安全性问题。
pub async fn publish_json<T: Serialize + Send + Sync>(
    mq: &impl MessageQueue,
    topic: &str,
    payload: &T,
) -> Result<(), MqError> {
    let bytes = serde_json::to_vec(payload).map_err(|e| MqError::SerializeFailed(e.to_string()))?;
    mq.publish(topic, &bytes).await
}

// ============ Noop 实现 ============

/// 空实现消息队列（默认关闭）
///
/// `publish` 不报错也不执行，`subscribe` 不订阅
pub struct NoopMessageQueue;

impl NoopMessageQueue {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MessageQueue for NoopMessageQueue {
    async fn publish(&self, _topic: &str, _payload: &[u8]) -> Result<(), MqError> {
        Ok(())
    }

    async fn subscribe<F, Fut>(&self, _topic: &str, _handler: F) -> Result<(), MqError>
    where
        F: Fn(Vec<u8>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), MqError>> + Send + 'static,
    {
        Ok(())
    }

    async fn health_check(&self) -> bool {
        true
    }
}

impl Default for NoopMessageQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ============ InMemory 实现 ============

/// 内存消息队列（仅用于开发/测试）
///
/// 基于 tokio::broadcast 实现，每个 topic 一个广播频道。
/// 限制：不持久化、不跨进程、重启即丢失。
pub struct InMemoryMessageQueue {
    topics: RwLock<HashMap<String, broadcast::Sender<Vec<u8>>>>,
}

impl InMemoryMessageQueue {
    /// 创建新的内存消息队列
    pub fn new() -> Self {
        Self {
            topics: RwLock::new(HashMap::new()),
        }
    }

    /// 获取或创建 topic 的 broadcast sender
    async fn get_or_create_topic(&self, topic: &str) -> broadcast::Sender<Vec<u8>> {
        let topics = self.topics.read().await;
        if let Some(sender) = topics.get(topic) {
            return sender.clone();
        }
        drop(topics);

        let mut topics = self.topics.write().await;
        if let Some(sender) = topics.get(topic) {
            return sender.clone();
        }
        let (tx, _) = broadcast::channel(256);
        topics.insert(topic.to_string(), tx.clone());
        tx
    }
}

#[async_trait]
impl MessageQueue for InMemoryMessageQueue {
    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), MqError> {
        let sender = self.get_or_create_topic(topic).await;
        sender
            .send(payload.to_vec())
            .map(|_| ())
            .map_err(|e| MqError::PublishFailed(format!("broadcast send failed: {}", e)))
    }

    async fn subscribe<F, Fut>(&self, topic: &str, handler: F) -> Result<(), MqError>
    where
        F: Fn(Vec<u8>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), MqError>> + Send + 'static,
    {
        let sender = self.get_or_create_topic(topic).await;
        let mut rx = sender.subscribe();

        let topic_owned = topic.to_string();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        if let Err(e) = handler(msg).await {
                            tracing::warn!("消息处理失败 (topic={}): {}", topic_owned, e);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("消息滞后 {} 条 (topic={})", n, topic_owned);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("消息频道已关闭 (topic={})", topic_owned);
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    async fn health_check(&self) -> bool {
        true
    }
}

impl Default for InMemoryMessageQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ============ Kafka 实现 ============

/// Kafka 消息队列（生产级）
///
/// 使用 rdkafka 库，需要系统安装 librdkafka。
/// 启用方式：在 Cargo.toml 中启用 `kafka` feature。
#[cfg(feature = "kafka")]
pub struct KafkaMessageQueue {
    producer: rdkafka::producer::FutureProducer,
    consumer_group: String,
    bootstrap_servers: String,
}

#[cfg(feature = "kafka")]
impl KafkaMessageQueue {
    /// 创建 Kafka 消息队列
    pub fn new(bootstrap_servers: &str, consumer_group: &str) -> Result<Self, MqError> {
        use rdkafka::config::ClientConfig;

        let producer: rdkafka::producer::FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", bootstrap_servers)
            .set("message.timeout.ms", "5000")
            .create()
            .map_err(|e| MqError::PublishFailed(format!("create producer: {}", e)))?;

        Ok(Self {
            producer,
            consumer_group: consumer_group.to_string(),
            bootstrap_servers: bootstrap_servers.to_string(),
        })
    }
}

#[cfg(feature = "kafka")]
#[async_trait]
impl MessageQueue for KafkaMessageQueue {
    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), MqError> {
        use rdkafka::producer::FutureRecord;

        self.producer
            .send(
                FutureRecord::to(topic).payload(payload).key(""),
                std::time::Duration::from_secs(5),
            )
            .await
            .map_err(|(e, _)| MqError::PublishFailed(format!("kafka send: {}", e)))?;

        Ok(())
    }

    async fn subscribe<F, Fut>(&self, topic: &str, handler: F) -> Result<(), MqError>
    where
        F: Fn(Vec<u8>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), MqError>> + Send + 'static,
    {
        use rdkafka::config::ClientConfig;
        use rdkafka::consumer::{Consumer, StreamConsumer};
        use tokio_stream::StreamExt;

        let consumer: StreamConsumer = ClientConfig::new()
            .set("bootstrap.servers", &self.bootstrap_servers)
            .set("group.id", &self.consumer_group)
            .set("enable.auto.commit", "true")
            .set("auto.offset.reset", "earliest")
            .create()
            .map_err(|e| MqError::SubscribeFailed(format!("create consumer: {}", e)))?;

        consumer
            .subscribe(&[topic])
            .map_err(|e| MqError::SubscribeFailed(format!("subscribe: {}", e)))?;

        let topic_owned = topic.to_string();
        tokio::spawn(async move {
            let mut stream = consumer.stream();
            while let Some(result) = stream.next().await {
                match result {
                    Ok(msg) => {
                        if let Some(payload) = msg.payload() {
                            if let Err(e) = handler(payload.to_vec()).await {
                                tracing::warn!("消息处理失败 (topic={}): {}", topic_owned, e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Kafka 消费错误 (topic={}): {}", topic_owned, e);
                    }
                }
            }
        });

        Ok(())
    }

    async fn health_check(&self) -> bool {
        self.producer
            .client()
            .fetch_metadata(None, std::time::Duration::from_secs(10))
            .is_ok()
    }
}

// ============ 后端枚举（委托模式） ============

/// 消息队列后端（委托模式，解决 trait 对象安全问题）
///
/// 每个 variant 持有具体实现，通过委托实现 `MessageQueue` trait。
/// 使用者通过 `create_message_queue` 工厂函数创建，无需关心内部类型。
pub enum MqBackend {
    /// 不启用消息队列
    Noop(NoopMessageQueue),
    /// 内存消息队列（测试/开发）
    InMemory(InMemoryMessageQueue),
    /// Kafka（生产环境，需 feature = "kafka"）
    #[cfg(feature = "kafka")]
    Kafka(KafkaMessageQueue),
}

#[async_trait]
impl MessageQueue for MqBackend {
    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), MqError> {
        match self {
            MqBackend::Noop(inner) => inner.publish(topic, payload).await,
            MqBackend::InMemory(inner) => inner.publish(topic, payload).await,
            #[cfg(feature = "kafka")]
            MqBackend::Kafka(inner) => inner.publish(topic, payload).await,
        }
    }

    async fn subscribe<F, Fut>(&self, topic: &str, handler: F) -> Result<(), MqError>
    where
        F: Fn(Vec<u8>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), MqError>> + Send + 'static,
    {
        match self {
            MqBackend::Noop(inner) => inner.subscribe(topic, handler).await,
            MqBackend::InMemory(inner) => inner.subscribe(topic, handler).await,
            #[cfg(feature = "kafka")]
            MqBackend::Kafka(inner) => inner.subscribe(topic, handler).await,
        }
    }

    async fn health_check(&self) -> bool {
        match self {
            MqBackend::Noop(inner) => inner.health_check().await,
            MqBackend::InMemory(inner) => inner.health_check().await,
            #[cfg(feature = "kafka")]
            MqBackend::Kafka(inner) => inner.health_check().await,
        }
    }
}

impl Default for MqBackend {
    fn default() -> Self {
        MqBackend::Noop(NoopMessageQueue::new())
    }
}

/// 创建消息队列实例
pub fn create_message_queue() -> MqBackend {
    // 默认使用 Noop，具体后端由配置决定
    MqBackend::Noop(NoopMessageQueue::new())
}

/// 创建内存消息队列（测试/开发用）
pub fn create_in_memory_mq() -> MqBackend {
    MqBackend::InMemory(InMemoryMessageQueue::new())
}

/// 创建 Kafka 消息队列（生产用）
#[cfg(feature = "kafka")]
pub fn create_kafka_mq(
    bootstrap_servers: &str,
    consumer_group: &str,
) -> Result<MqBackend, MqError> {
    Ok(MqBackend::Kafka(KafkaMessageQueue::new(
        bootstrap_servers,
        consumer_group,
    )?))
}

// ============ 测试 ============

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_in_memory_publish_subscribe() {
        let mq = InMemoryMessageQueue::new();

        // 先订阅
        mq.subscribe("test.topic", |msg| async move {
            let text = String::from_utf8(msg).unwrap();
            assert_eq!(text, "hello world");
            Ok(())
        })
        .await
        .unwrap();

        // 后发布
        mq.publish("test.topic", b"hello world").await.unwrap();

        // 给异步处理一点时间
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_publish_json() {
        let mq = InMemoryMessageQueue::new();

        mq.subscribe("json.topic", |msg| async move {
            let val: serde_json::Value = serde_json::from_slice(&msg)
                .map_err(|e| MqError::DeserializeFailed(e.to_string()))?;
            assert_eq!(val["key"], "value");
            Ok(())
        })
        .await
        .unwrap();

        publish_json(&mq, "json.topic", &serde_json::json!({"key": "value"}))
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_noop_does_nothing() {
        let mq = NoopMessageQueue::new();
        // 不应 panic
        mq.publish("any", b"data").await.unwrap();
        mq.subscribe("any", |msg: Vec<u8>| async move {
            let _ = msg;
            Ok(())
        })
        .await
        .unwrap();
        assert!(mq.health_check().await);
    }

    #[tokio::test]
    async fn test_health_check() {
        assert!(InMemoryMessageQueue::new().health_check().await);
        assert!(NoopMessageQueue::new().health_check().await);
    }
}
