use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use ryframe_adapters::RedisClient;
use ryframe_api::message_socket::MessageHub;
use ryframe_application::{
    AUTHORIZATION_CHANGED_REDIS_CHANNEL,
    ports::tenants::TenantRuntimeReadPort,
    system::{MESSAGE_DISPATCH_REDIS_CHANNEL, MessageService},
};
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeupKind {
    AuthorizationChanged,
    Message,
}

/// 在 Redis 可用时订阅跨实例唤醒信号；掉线后按指数退避重连。
pub fn spawn(
    hub: &MessageHub,
    redis: Option<RedisClient>,
    service: Arc<MessageService>,
    tenant_data: Arc<dyn TenantRuntimeReadPort>,
    enabled: bool,
) -> Option<JoinHandle<()>> {
    ryframe_api::metrics::set_message_redis_listener_connected(false);
    if !enabled {
        return None;
    }
    let redis = redis?;
    let hub = hub.clone();
    Some(tokio::spawn(async move {
        let authorization_channel = redis.scoped_channel(AUTHORIZATION_CHANGED_REDIS_CHANNEL);
        let message_channel = redis.scoped_channel(MESSAGE_DISPATCH_REDIS_CHANNEL);
        let mut retry_seconds = 1_u64;
        let mut degraded = false;
        loop {
            match redis
                .subscribe_many(&[
                    MESSAGE_DISPATCH_REDIS_CHANNEL,
                    AUTHORIZATION_CHANGED_REDIS_CHANNEL,
                ])
                .await
            {
                Ok(subscription) => {
                    ryframe_api::metrics::set_message_redis_listener_connected(true);
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
                        let channel = raw.get_channel_name();
                        let Ok(payload) = raw.get_payload::<String>() else {
                            tracing::warn!("收到无法解析的消息唤醒负载");
                            continue;
                        };
                        match classify_channel(channel, &authorization_channel, &message_channel) {
                            Some(WakeupKind::AuthorizationChanged) => {
                                hub.deliver_authorization_change(tenant_data.as_ref(), &payload)
                                    .await;
                            }
                            Some(WakeupKind::Message) => {
                                let Ok(message_id) = payload.parse::<i64>() else {
                                    tracing::warn!("收到无效的消息唤醒标识");
                                    continue;
                                };
                                if let Err(error) = hub.deliver_message(&service, message_id).await
                                {
                                    tracing::warn!(%error, message_id, "消息在线投递失败，将由收件箱补拉恢复");
                                }
                            }
                            None => tracing::warn!(channel, "收到未知的实时通知频道"),
                        }
                    }
                    ryframe_api::metrics::set_message_redis_listener_connected(false);
                    if !degraded {
                        tracing::warn!("消息 WebSocket Redis 订阅已中断");
                        degraded = true;
                    } else {
                        tracing::debug!("消息 WebSocket Redis 订阅仍不可用");
                    }
                }
                Err(error) => {
                    ryframe_api::metrics::set_message_redis_listener_connected(false);
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

pub fn classify_channel(
    channel: &str,
    authorization_channel: &str,
    message_channel: &str,
) -> Option<WakeupKind> {
    if channel == authorization_channel {
        Some(WakeupKind::AuthorizationChanged)
    } else if channel == message_channel {
        Some(WakeupKind::Message)
    } else {
        None
    }
}
