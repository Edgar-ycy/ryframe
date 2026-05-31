//! 异步任务队列
//!
//! 基于 Redis List 的轻量级任务队列，支持：
//! - Redis 模式：LPUSH 入队 / BRPOP 出队（阻塞等待），支持分布式消费
//! - 内存模式（Redis 不可用降级）：tokio mpsc channel
//! - 延迟任务：使用 Redis Sorted Set（ZADD score=执行时间戳）
//!
//! 典型用途：后台邮件发送、报表生成、数据导出等异步任务。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::RedisClient;

/// 任务消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMessage {
    /// 任务类型标识
    pub task_type: String,
    /// JSON 序列化的任务载荷
    pub payload: String,
    /// 任务创建时间戳（毫秒）
    pub created_at: i64,
    /// 最大重试次数
    pub max_retries: u32,
    /// 当前重试次数
    pub retry_count: u32,
}

/// 异步任务队列
///
/// FIFO 队列，Redis 模式支持多消费者分布式消费。
#[derive(Clone)]
pub struct TaskQueue {
    redis: Option<RedisClient>,
    /// 内存模式发送端
    tx: Option<mpsc::UnboundedSender<String>>,
    /// 内存模式接收端（仅在消费端使用）
    rx: Option<Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>>>,
    /// 队列名称
    queue_name: String,
}

use std::sync::Arc;

impl TaskQueue {
    /// 创建任务队列
    ///
    /// Redis 模式下，同一 queue_name 的多个实例共享队列。
    /// 内存模式下，生产者和消费者需使用同一实例（或 clone）。
    pub fn new(redis: Option<RedisClient>, queue_name: &str) -> Self {
        let (tx, rx) = if redis.is_none() {
            let (tx, rx) = mpsc::unbounded_channel();
            (Some(tx), Some(Arc::new(tokio::sync::Mutex::new(rx))))
        } else {
            (None, None)
        };

        Self {
            redis,
            tx,
            rx,
            queue_name: queue_name.to_string(),
        }
    }

    /// 获取 Redis key
    fn list_key(&self) -> String {
        format!("task_queue:{}", self.queue_name)
    }

    /// 入队任务（非阻塞）
    ///
    /// 将任务推入队列尾部，立即返回。
    pub async fn enqueue(&self, task: &TaskMessage) -> Result<(), String> {
        if let Some(ref redis) = self.redis {
            let payload =
                serde_json::to_string(task).map_err(|e| format!("序列化任务失败: {}", e))?;
            redis::cmd("LPUSH")
                .arg(self.list_key())
                .arg(&payload)
                .query_async(&mut redis.conn().clone())
                .await
                .map_err(|e| format!("Redis LPUSH 失败: {}", e))
        } else if let Some(ref tx) = self.tx {
            let payload =
                serde_json::to_string(task).map_err(|e| format!("序列化任务失败: {}", e))?;
            tx.send(payload)
                .map_err(|e| format!("Channel send 失败: {}", e))
        } else {
            Err("任务队列未初始化（缺少 Redis 且无内存 channel）".into())
        }
    }

    /// 出队任务（阻塞等待）
    ///
    /// Redis 模式使用 BRPOP 阻塞等待新任务。
    /// 内存模式使用 mpsc receiver 等待。
    ///
    /// `timeout_secs` - 最大等待秒数，0 表示永久等待
    pub async fn dequeue(&self, timeout_secs: u64) -> Result<Option<TaskMessage>, String> {
        if let Some(ref redis) = self.redis {
            if timeout_secs == 0 {
                // 永久阻塞
                let result: Option<(String, String)> = redis::cmd("BRPOP")
                    .arg(self.list_key())
                    .arg(0)
                    .query_async(&mut redis.conn().clone())
                    .await
                    .map_err(|e| format!("Redis BRPOP 失败: {}", e))?;

                match result {
                    Some((_, payload)) => {
                        let task: TaskMessage = serde_json::from_str(&payload)
                            .map_err(|e| format!("反序列化任务失败: {}", e))?;
                        Ok(Some(task))
                    }
                    None => Ok(None),
                }
            } else {
                // 带超时的 BRPOP
                let result: Option<(String, String)> = redis::cmd("BRPOP")
                    .arg(self.list_key())
                    .arg(timeout_secs)
                    .query_async(&mut redis.conn().clone())
                    .await
                    .map_err(|e| format!("Redis BRPOP 失败: {}", e))?;

                match result {
                    Some((_, payload)) => {
                        let task: TaskMessage = serde_json::from_str(&payload)
                            .map_err(|e| format!("反序列化任务失败: {}", e))?;
                        Ok(Some(task))
                    }
                    None => Ok(None), // 超时
                }
            }
        } else if let Some(ref rx) = self.rx {
            let mut rx = rx.lock().await;
            if timeout_secs == 0 {
                match rx.recv().await {
                    Some(payload) => {
                        let task: TaskMessage = serde_json::from_str(&payload)
                            .map_err(|e| format!("反序列化任务失败: {}", e))?;
                        Ok(Some(task))
                    }
                    None => Ok(None), // channel 已关闭
                }
            } else {
                match tokio::time::timeout(Duration::from_secs(timeout_secs), rx.recv()).await {
                    Ok(Some(payload)) => {
                        let task: TaskMessage = serde_json::from_str(&payload)
                            .map_err(|e| format!("反序列化任务失败: {}", e))?;
                        Ok(Some(task))
                    }
                    Ok(None) => Ok(None),
                    Err(_) => Ok(None), // 超时
                }
            }
        } else {
            Err("任务队列未初始化".into())
        }
    }

    /// 查询队列长度
    pub async fn len(&self) -> Result<usize, String> {
        if let Some(ref redis) = self.redis {
            let result: i64 = redis::cmd("LLEN")
                .arg(self.list_key())
                .query_async(&mut redis.conn().clone())
                .await
                .map_err(|e| format!("Redis LLEN 失败: {}", e))?;
            Ok(result as usize)
        } else {
            // 内存模式无法精确获取队列长度
            Ok(0)
        }
    }

    /// 检查队列是否为空
    pub async fn is_empty(&self) -> Result<bool, String> {
        self.len().await.map(|l| l == 0)
    }

    /// 入队延迟任务
    ///
    /// 使用 Redis Sorted Set，score 为执行时间戳（毫秒）。
    /// 需要配合 `dequeue_delayed` 轮询取出到期任务。
    pub async fn enqueue_delayed(
        &self,
        task: &TaskMessage,
        execute_at_ms: i64,
    ) -> Result<(), String> {
        if let Some(ref redis) = self.redis {
            let payload =
                serde_json::to_string(task).map_err(|e| format!("序列化任务失败: {}", e))?;
            let delay_key = format!("{}:delayed", self.list_key());
            redis::cmd("ZADD")
                .arg(&delay_key)
                .arg(execute_at_ms)
                .arg(&payload)
                .query_async(&mut redis.conn().clone())
                .await
                .map_err(|e| format!("Redis ZADD 失败: {}", e))
        } else {
            Err("延迟任务仅支持 Redis 模式".into())
        }
    }

    /// 取出到期的延迟任务并移入主队列
    ///
    /// 查询 score <= now_ms 的任务，批量移入主队列。
    /// 使用 Lua 脚本保证原子性。
    pub async fn poll_delayed(&self, now_ms: i64) -> Result<Vec<TaskMessage>, String> {
        if let Some(ref redis) = self.redis {
            let delay_key = format!("{}:delayed", self.list_key());
            let list_key = self.list_key();

            // Lua 脚本：原子地取出到期任务并移入队列
            let script = redis::Script::new(
                r#"
                local items = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', ARGV[1])
                if #items > 0 then
                    for i, v in ipairs(items) do
                        redis.call('LPUSH', KEYS[2], v)
                    end
                    redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', ARGV[1])
                end
                return items
                "#,
            );

            let result: Vec<String> = script
                .key(&delay_key)
                .key(&list_key)
                .arg(now_ms)
                .invoke_async(&mut redis.conn().clone())
                .await
                .map_err(|e| format!("延迟任务轮询失败: {}", e))?;

            let tasks: Vec<TaskMessage> = result
                .iter()
                .filter_map(|payload| serde_json::from_str(payload).ok())
                .collect();

            Ok(tasks)
        } else {
            Err("延迟任务仅支持 Redis 模式".into())
        }
    }
}
