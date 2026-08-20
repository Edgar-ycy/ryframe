use std::sync::Arc;

pub trait UploadCircuitBreaker: Send + Sync {
    fn allow_request(&self) -> bool;
    fn record_success(&self);
    fn record_failure(&self);
    fn state_label(&self) -> &'static str;
}

#[derive(Clone)]
pub struct RuntimeComponents {
    pub upload_circuit_breaker: Arc<dyn UploadCircuitBreaker>,
}

impl RuntimeComponents {
    pub fn new(upload_circuit_breaker: Arc<dyn UploadCircuitBreaker>) -> Self {
        Self {
            upload_circuit_breaker,
        }
    }
}
