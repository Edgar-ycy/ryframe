use serde::Deserialize;

fn default_enabled() -> bool {
    true
}

fn default_capacity() -> u32 {
    100
}

fn default_refill() -> u32 {
    20
}

/// 限流配置
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_capacity")]
    pub capacity: u32,
    #[serde(default = "default_refill")]
    pub refill_per_sec: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            capacity: 100,
            refill_per_sec: 20,
        }
    }
}
