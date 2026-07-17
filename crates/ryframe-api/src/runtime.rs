use std::sync::Arc;

use ryframe_core::{
    DistributedLock, RedisClient, create_distributed_lock, resilience::CircuitBreaker,
};

#[derive(Clone)]
pub struct RuntimeComponents {
    pub distributed_lock: Arc<dyn DistributedLock>,
    pub upload_circuit_breaker: Arc<CircuitBreaker>,
}

impl RuntimeComponents {
    pub fn new(redis: Option<RedisClient>) -> Self {
        Self {
            distributed_lock: create_distributed_lock(redis.as_ref()),
            upload_circuit_breaker: Arc::new(CircuitBreaker::default_config()),
        }
    }
}
