use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use ryframe_kernel::AppResult;
use tokio::sync::Mutex;

struct CaptchaEntry {
    answer: String,
    created_at: Instant,
}

#[async_trait::async_trait]
pub trait CaptchaStore: Send + Sync {
    async fn set(&self, id: String, answer: String) -> AppResult<()>;

    async fn verify(&self, id: &str, code: &str) -> AppResult<bool>;
}

#[derive(Clone)]
pub struct InMemoryCaptchaStore {
    inner: Arc<Mutex<HashMap<String, CaptchaEntry>>>,
    ttl: Duration,
}

impl InMemoryCaptchaStore {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    pub fn spawn_gc(&self) {
        let inner = Arc::clone(&self.inner);
        let ttl = self.ttl;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(120));
            loop {
                interval.tick().await;
                inner
                    .lock()
                    .await
                    .retain(|_, entry| entry.created_at.elapsed() <= ttl);
            }
        });
    }
}

#[async_trait::async_trait]
impl CaptchaStore for InMemoryCaptchaStore {
    async fn set(&self, id: String, answer: String) -> AppResult<()> {
        self.inner.lock().await.insert(
            id,
            CaptchaEntry {
                answer,
                created_at: Instant::now(),
            },
        );
        Ok(())
    }

    async fn verify(&self, id: &str, code: &str) -> AppResult<bool> {
        let Some(entry) = self.inner.lock().await.remove(id) else {
            return Ok(false);
        };
        Ok(entry.created_at.elapsed() <= self.ttl && entry.answer.eq_ignore_ascii_case(code))
    }
}
