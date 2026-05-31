//! Redis 缓存监控模块
//!
//! 提供 Redis 缓存信息查询，包括：
//! - 服务器信息（INFO 命令解析）
//! - 键统计（DBSIZE）
//! - 内存分析

use std::collections::HashMap;

use ryframe_core::RedisClient;
use serde::Serialize;

/// 缓存信息响应
#[derive(Debug, Serialize)]
pub struct CacheInfo {
    /// Redis 是否可用
    pub available: bool,
    /// 缓存模式: "redis" 或 "memory"
    pub mode: String,
    /// Redis 服务器信息（仅 Redis 模式）
    pub server: Option<RedisServerInfo>,
    /// 键统计
    pub keys: CacheKeysInfo,
    /// 内存信息
    pub memory: Option<RedisMemoryInfo>,
}

/// Redis 服务器基本信息
#[derive(Debug, Serialize)]
pub struct RedisServerInfo {
    /// Redis 版本
    pub version: String,
    /// 运行模式
    pub mode: String,
    /// 操作系统
    pub os: String,
    /// 运行天数
    pub uptime_days: u64,
    /// 连接数
    pub connected_clients: u64,
}

/// 缓存键统计
#[derive(Debug, Serialize)]
pub struct CacheKeysInfo {
    /// 当前数据库键总数
    pub total_keys: u64,
    /// 在线用户会话数
    pub online_users: u64,
    /// 验证码数
    pub captchas: u64,
    /// 限流计数器数
    pub rate_limits: u64,
    /// 字典缓存数
    pub dict_cache: u64,
    /// 配置缓存数
    pub config_cache: u64,
}

/// Redis 内存信息
#[derive(Debug, Serialize)]
pub struct RedisMemoryInfo {
    /// 已用内存（人类可读）
    pub used_memory_human: String,
    /// 内存峰值（人类可读）
    pub used_memory_peak_human: String,
    /// 内存碎片率
    pub mem_fragmentation_ratio: f64,
    /// 已用内存（字节）
    pub used_memory: u64,
}

/// 解析 Redis INFO 输出为 HashMap
fn parse_info_map(info: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in info.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            map.insert(key.to_string(), value.trim().to_string());
        }
    }
    map
}

/// 获取缓存信息
pub async fn get_cache_info(client: Option<&RedisClient>) -> CacheInfo {
    match client {
        Some(redis) => get_redis_cache_info(redis).await,
        None => get_memory_cache_info().await,
    }
}

/// Redis 模式缓存信息
async fn get_redis_cache_info(client: &RedisClient) -> CacheInfo {
    // 获取 INFO 输出
    let info_result: Result<String, _> = {
        let mut conn = client.conn().clone();
        redis::cmd("INFO").query_async(&mut conn).await
    };

    let info = match info_result {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("Redis INFO 命令失败: {}", e);
            return CacheInfo {
                available: false,
                mode: "redis".to_string(),
                server: None,
                keys: CacheKeysInfo {
                    total_keys: 0,
                    online_users: 0,
                    captchas: 0,
                    rate_limits: 0,
                    dict_cache: 0,
                    config_cache: 0,
                },
                memory: None,
            };
        }
    };

    let info_map = parse_info_map(&info);

    // 解析服务器信息
    let server = RedisServerInfo {
        version: info_map
            .get("redis_version")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        mode: info_map
            .get("redis_mode")
            .cloned()
            .unwrap_or_else(|| "standalone".to_string()),
        os: info_map
            .get("os")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        uptime_days: info_map
            .get("uptime_in_days")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        connected_clients: info_map
            .get("connected_clients")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    };

    // 解析内存信息
    let memory = RedisMemoryInfo {
        used_memory_human: info_map
            .get("used_memory_human")
            .cloned()
            .unwrap_or_else(|| "0B".to_string()),
        used_memory_peak_human: info_map
            .get("used_memory_peak_human")
            .cloned()
            .unwrap_or_else(|| "0B".to_string()),
        mem_fragmentation_ratio: info_map
            .get("mem_fragmentation_ratio")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
        used_memory: info_map
            .get("used_memory")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    };

    // 获取 DBSIZE
    let total_keys = {
        let mut conn = client.conn().clone();
        let result: Result<u64, _> = redis::cmd("DBSIZE").query_async(&mut conn).await;
        result.unwrap_or(0)
    };

    // 统计各前缀的键数
    let online_users = count_keys(client, "online_user:*").await;
    let captchas = count_keys(client, "ryframe:captcha:*").await;
    let rate_limits = count_keys(client, "rate_limit:*").await;
    let dict_cache = count_keys(client, "sys_dict:data:*").await;
    let config_cache = count_keys(client, "sys_config:key:*").await;

    CacheInfo {
        available: true,
        mode: "redis".to_string(),
        server: Some(server),
        keys: CacheKeysInfo {
            total_keys,
            online_users,
            captchas,
            rate_limits,
            dict_cache,
            config_cache,
        },
        memory: Some(memory),
    }
}

/// 内存模式缓存信息（有限信息）
async fn get_memory_cache_info() -> CacheInfo {
    CacheInfo {
        available: true,
        mode: "memory".to_string(),
        server: None,
        keys: CacheKeysInfo {
            total_keys: 0,
            online_users: 0,
            captchas: 0,
            rate_limits: 0,
            dict_cache: 0,
            config_cache: 0,
        },
        memory: None,
    }
}

/// 统计匹配模式的键数量
async fn count_keys(client: &RedisClient, pattern: &str) -> u64 {
    match client.keys(pattern).await {
        Ok(keys) => keys.len() as u64,
        Err(_) => 0,
    }
}

/// 获取 Redis 命令统计信息
pub async fn get_cache_command_stats(client: &RedisClient) -> Option<serde_json::Value> {
    let info_result: Result<String, _> = {
        let mut conn = client.conn().clone();
        redis::cmd("INFO")
            .arg("commandstats")
            .query_async(&mut conn)
            .await
    };

    match info_result {
        Ok(info) => {
            let mut stats = serde_json::Map::new();
            for line in info.lines() {
                if line.starts_with("cmdstat_")
                    && let Some((cmd, data)) = line.split_once(':')
                {
                    let cmd_name = cmd.strip_prefix("cmdstat_").unwrap_or(cmd);
                    stats.insert(
                        cmd_name.to_string(),
                        serde_json::Value::String(data.to_string()),
                    );
                }
            }
            Some(serde_json::Value::Object(stats))
        }
        Err(_) => None,
    }
}
