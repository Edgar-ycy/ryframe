//! Redis 缓存监控模块
//!
//! 提供 Redis 缓存信息查询，包括：
//! - 服务器信息（INFO 命令解析）
//! - 键统计（DBSIZE）
//! - 内存分析

use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
};

use ryframe_core::RedisClient;
use serde::Serialize;
use utoipa::ToSchema;

/// 缓存信息响应
#[derive(Debug, Serialize, ToSchema)]
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
#[derive(Debug, Serialize, ToSchema)]
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
#[derive(Debug, Serialize, ToSchema)]
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

impl CacheKeysInfo {
    fn empty() -> Self {
        Self {
            total_keys: 0,
            online_users: 0,
            captchas: 0,
            rate_limits: 0,
            dict_cache: 0,
            config_cache: 0,
        }
    }
}

/// Redis 内存信息
#[derive(Debug, Serialize, ToSchema)]
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

fn info_string(map: &HashMap<String, String>, key: &str, default: &str) -> String {
    map.get(key).cloned().unwrap_or_else(|| default.to_string())
}

fn info_parse<T>(map: &HashMap<String, String>, key: &str, default: T) -> T
where
    T: FromStr,
{
    map.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

async fn redis_info(client: &RedisClient, section: Option<&str>) -> redis::RedisResult<String> {
    let mut conn = client.conn().clone();
    let mut cmd = redis::cmd("INFO");
    if let Some(section) = section {
        cmd.arg(section);
    }
    cmd.query_async(&mut conn).await
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
    let info_result = redis_info(client, None).await;

    let info = match info_result {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("Redis INFO 命令失败: {}", e);
            return CacheInfo {
                available: false,
                mode: "redis".to_string(),
                server: None,
                keys: CacheKeysInfo::empty(),
                memory: None,
            };
        }
    };

    let info_map = parse_info_map(&info);

    // 解析服务器信息
    let server = RedisServerInfo {
        version: info_string(&info_map, "redis_version", "unknown"),
        mode: info_string(&info_map, "redis_mode", "standalone"),
        os: info_string(&info_map, "os", "unknown"),
        uptime_days: info_parse(&info_map, "uptime_in_days", 0),
        connected_clients: info_parse(&info_map, "connected_clients", 0),
    };

    // 解析内存信息
    let memory = RedisMemoryInfo {
        used_memory_human: info_string(&info_map, "used_memory_human", "0B"),
        used_memory_peak_human: info_string(&info_map, "used_memory_peak_human", "0B"),
        mem_fragmentation_ratio: info_parse(&info_map, "mem_fragmentation_ratio", 0.0),
        used_memory: info_parse(&info_map, "used_memory", 0),
    };

    // 获取 DBSIZE
    let total_keys = {
        let mut conn = client.conn().clone();
        let result: Result<u64, _> = redis::cmd("DBSIZE").query_async(&mut conn).await;
        result.unwrap_or(0)
    };

    // 统计各前缀的键数
    let online_users = count_keys(client, "ryframe:v0.5:online-user:*").await;
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
        keys: CacheKeysInfo::empty(),
        memory: None,
    }
}

/// 统计匹配模式的键数量
async fn count_keys(client: &RedisClient, pattern: &str) -> u64 {
    match client.scan_keys(pattern).await {
        Ok(keys) => keys.len() as u64,
        Err(error) => {
            tracing::warn!(%error, pattern, "failed to scan Redis keys");
            0
        }
    }
}

/// 获取 Redis 命令统计信息
pub async fn get_cache_command_stats(client: &RedisClient) -> Option<BTreeMap<String, String>> {
    let info_result = redis_info(client, Some("commandstats")).await;

    match info_result {
        Ok(info) => {
            let mut stats = BTreeMap::new();
            for line in info.lines() {
                if line.starts_with("cmdstat_")
                    && let Some((cmd, data)) = line.split_once(':')
                {
                    let cmd_name = cmd.strip_prefix("cmdstat_").unwrap_or(cmd);
                    stats.insert(cmd_name.to_string(), data.to_string());
                }
            }
            Some(stats)
        }
        Err(_) => None,
    }
}
