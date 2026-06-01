//! 事件总线 —— 模块间解耦通信
//!
//! 支持：
//! - 类型安全的事件发布/订阅（基于 `TypeId` 路由）
//! - 一对多广播（单个事件 → 所有订阅者并发执行，通过 Arc 共享零拷贝）
//! - 函数式和 trait 两种订阅方式
//! - 同步/异步发布
//! - 订阅者错误隔离（单个处理失败不影响其他）
//!
//! # 使用示例
//!
//! ```
//! use ryframe_core::event_bus::{Event, EventBus};
//! use std::sync::Arc;
//!
//! #[derive(Debug, Clone)]
//! struct UserCreatedEvent { user_id: i64, username: String }
//! impl Event for UserCreatedEvent {}
//!
//! # #[tokio::main]
//! # async fn main() {
//! let bus = EventBus::new();
//! bus.subscribe_fn(|event: Arc<UserCreatedEvent>| async move {
//!     assert_eq!(event.username, "alice");
//!     Ok(())
//! });
//! bus.publish(UserCreatedEvent { user_id: 1, username: "alice".into() }).await;
//! # }
//! ```

use std::{
    any::{Any, TypeId},
    future::Future,
    pin::Pin,
    sync::Arc,
};

use async_trait::async_trait;
use dashmap::DashMap;
use tracing::{error, info};

// ============ 核心类型 ============

/// 事件 trait —— 所有业务事件必须实现此 trait
pub trait Event: Send + Sync + 'static {}

/// 事件处理结果
pub type EventResult = Result<(), String>;

/// 事件处理器 trait（结构体方式）
#[async_trait]
pub trait EventHandler<E: Event>: Send + Sync {
    /// 处理事件
    async fn handle(&self, event: Arc<E>) -> EventResult;
}

// 类型擦除后的处理器签名（通过 Arc 共享事件，无需深拷贝）
type ErasedHandler = Arc<
    dyn Fn(Arc<dyn Any + Send + Sync>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

// ============ 事件总线 ============

/// 事件总线
///
/// 线程安全、可克隆（内部使用 `Arc`）。
/// 通过 `publish()` 发布事件，所有订阅该事件类型的处理器将被并发调用。
/// 事件通过 `Arc` 在多个处理器间共享，无需 `Clone`。
#[derive(Clone)]
pub struct EventBus {
    handlers: Arc<DashMap<TypeId, Vec<ErasedHandler>>>,
}

impl EventBus {
    /// 创建新的事件总线
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(DashMap::new()),
        }
    }

    /// 订阅事件（trait 对象方式）
    ///
    /// # 示例
    /// ```
    /// # use ryframe_core::event_bus::{Event, EventBus, EventHandler, EventResult};
    /// # use std::sync::Arc;
    /// # use async_trait::async_trait;
    /// # #[derive(Debug, Clone)]
    /// # struct UserCreatedEvent { user_id: i64 }
    /// # impl Event for UserCreatedEvent {}
    /// struct LogHandler;
    /// #[async_trait]
    /// impl EventHandler<UserCreatedEvent> for LogHandler {
    ///     async fn handle(&self, _event: Arc<UserCreatedEvent>) -> EventResult {
    ///         Ok(())
    ///     }
    /// }
    /// # let bus = EventBus::new();
    /// bus.subscribe::<UserCreatedEvent, _>(LogHandler);
    /// ```
    pub fn subscribe<E: Event, H: EventHandler<E> + 'static>(&self, handler: H) {
        let handler_arc = Arc::new(handler);
        let erased: ErasedHandler = Arc::new(move |event_arc: Arc<dyn Any + Send + Sync>| {
            let h = handler_arc.clone();
            Box::pin(async move {
                match event_arc.downcast::<E>() {
                    Ok(concrete) => {
                        if let Err(err) = h.handle(concrete).await {
                            error!(
                                event_type = %std::any::type_name::<E>(),
                                error = %err,
                                "事件处理器执行失败"
                            );
                        }
                    }
                    Err(_) => {
                        error!(
                            event_type = %std::any::type_name::<E>(),
                            "事件类型下转型失败（内部错误）"
                        );
                    }
                }
            })
        });
        self.handlers
            .entry(TypeId::of::<E>())
            .or_default()
            .push(erased);

        info!(
            event_type = %std::any::type_name::<E>(),
            handler_count = self.handlers.get(&TypeId::of::<E>()).map(|v| v.len()).unwrap_or(0),
            "事件订阅已注册"
        );
    }

    /// 订阅事件（函数式方式）
    ///
    /// 处理器接收 `Arc<E>`，与其它处理器共享同一份事件数据，无需 Clone。
    ///
    /// # 示例
    /// ```
    /// # use ryframe_core::event_bus::{Event, EventBus};
    /// # use std::sync::Arc;
    /// # #[derive(Debug, Clone)]
    /// # struct UserCreatedEvent { username: String }
    /// # impl Event for UserCreatedEvent {}
    /// # let bus = EventBus::new();
    /// bus.subscribe_fn(|event: Arc<UserCreatedEvent>| async move {
    ///     assert_eq!(event.username, "alice");
    ///     Ok(())
    /// });
    /// ```
    pub fn subscribe_fn<E, F, Fut>(&self, f: F)
    where
        E: Event + 'static,
        F: Fn(Arc<E>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = EventResult> + Send + 'static,
    {
        let handler = Arc::new(f);
        let erased: ErasedHandler = Arc::new(move |event_arc: Arc<dyn Any + Send + Sync>| {
            let h = handler.clone();
            Box::pin(async move {
                match event_arc.downcast::<E>() {
                    Ok(concrete) => {
                        if let Err(err) = h(concrete).await {
                            error!(
                                event_type = %std::any::type_name::<E>(),
                                error = %err,
                                "事件处理器执行失败"
                            );
                        }
                    }
                    Err(_) => {
                        error!(
                            event_type = %std::any::type_name::<E>(),
                            "事件类型下转型失败（内部错误）"
                        );
                    }
                }
            })
        });
        self.handlers
            .entry(TypeId::of::<E>())
            .or_default()
            .push(erased);

        info!(
            event_type = %std::any::type_name::<E>(),
            handler_count = self.handlers.get(&TypeId::of::<E>()).map(|v| v.len()).unwrap_or(0),
            "事件订阅已注册"
        );
    }

    /// 发布事件（fire-and-forget）
    ///
    /// 所有订阅该事件类型的处理器将被并发调用（通过 `tokio::spawn`）。
    /// 单个处理器失败仅记录日志，不影响其他处理器或调用方。
    ///
    /// # 示例
    /// ```
    /// # use ryframe_core::event_bus::{Event, EventBus};
    /// # #[derive(Debug, Clone)]
    /// # struct UserCreatedEvent { user_id: i64, username: String }
    /// # impl Event for UserCreatedEvent {}
    /// # #[tokio::main]
    /// # async fn main() {
    /// let bus = EventBus::new();
    /// bus.publish(UserCreatedEvent { user_id: 1, username: "alice".into() }).await;
    /// # }
    /// ```
    pub async fn publish<E: Event>(&self, event: E) {
        let type_id = TypeId::of::<E>();
        if let Some(handlers_ref) = self.handlers.get(&type_id) {
            let handlers: Vec<ErasedHandler> = handlers_ref.value().clone();

            if handlers.is_empty() {
                return;
            }

            // 通过 Arc 共享事件，所有处理器读取同一份数据（零拷贝）
            let event_arc: Arc<dyn Any + Send + Sync> = Arc::new(event);

            // 并发触发所有处理器
            let mut handles = Vec::with_capacity(handlers.len());
            for (i, handler) in handlers.into_iter().enumerate() {
                let event_clone = event_arc.clone();
                let handler = handler.clone();

                let join_handle = tokio::spawn(async move {
                    handler(event_clone).await;
                });
                handles.push((i, join_handle));
            }

            let handler_count = handles.len();
            let mut failed = 0u32;

            for (i, handle) in handles {
                if let Err(err) = handle.await {
                    failed += 1;
                    error!(
                        event_type = %std::any::type_name::<E>(),
                        handler_index = i,
                        error = %err,
                        "事件处理器 panic"
                    );
                }
            }

            tracing::debug!(
                event_type = %std::any::type_name::<E>(),
                total = handler_count,
                failed = failed,
                "事件发布完成"
            );
        }
    }

    /// 同步发布事件（在当前任务中执行所有处理器，不 spawn）
    ///
    /// 适用于需要在事务中同步处理事件的场景。
    pub async fn publish_sync<E: Event>(&self, event: E) {
        let type_id = TypeId::of::<E>();
        if let Some(handlers_ref) = self.handlers.get(&type_id) {
            let handlers: Vec<ErasedHandler> = handlers_ref.value().clone();

            let event_arc: Arc<dyn Any + Send + Sync> = Arc::new(event);

            for (i, handler) in handlers.into_iter().enumerate() {
                handler(event_arc.clone()).await;
                tracing::trace!(
                    event_type = %std::any::type_name::<E>(),
                    handler_index = i,
                    "同步事件处理完成"
                );
            }
        }
    }

    /// 获取指定事件类型的订阅者数量
    pub fn subscriber_count<E: Event>(&self) -> usize {
        self.handlers
            .get(&TypeId::of::<E>())
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// 清空所有订阅
    pub fn clear(&self) {
        self.handlers.clear();
        info!("事件总线已清空所有订阅");
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.handlers.len();
        let total_subscribers: usize = self.handlers.iter().map(|entry| entry.value().len()).sum();
        f.debug_struct("EventBus")
            .field("event_types", &count)
            .field("total_subscribers", &total_subscribers)
            .finish()
    }
}
