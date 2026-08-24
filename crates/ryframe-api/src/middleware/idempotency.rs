//! HTTP 写请求幂等保留与响应重放。

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::{DashMap, mapref::entry::Entry};
use ryframe_auth::RequestPrincipal;
use serde::{Deserialize, Serialize};

mod response;

use response::*;

use crate::metrics::{record_idempotency_conflict, record_redis_degraded};

const DEFAULT_PROCESSING_TTL_SECS: u64 = 30;
const MAX_REQUEST_BYTES: usize = 10 * 1024 * 1024;
const MAX_CACHED_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct IdempotencyState {
    store: Option<Arc<dyn HttpIdempotencyStore>>,
    local: Arc<DashMap<String, LocalRecord>>,
    completed_ttl_secs: u64,
    processing_ttl_secs: u64,
}

#[derive(Clone)]
struct LocalRecord {
    fingerprint: String,
    state: LocalState,
    expires_at: Instant,
}

#[derive(Clone)]
enum LocalState {
    Processing,
    Completed(CachedResponse),
    NonReplayable,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: Vec<CachedHeader>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CachedHeader {
    pub name: String,
    pub value: Vec<u8>,
}

pub enum Reservation {
    Acquired,
    Processing,
    Conflict,
    Completed(CachedResponse),
    NonReplayable,
}

#[derive(Debug, Eq, PartialEq)]
pub enum StoredIdempotencyReservation {
    Acquired,
    Processing,
    Conflict,
    Completed(String),
    NonReplayable,
}

/// HTTP 幂等状态的外部持久化端口。
#[async_trait::async_trait]
pub trait HttpIdempotencyStore: Send + Sync {
    async fn reserve(
        &self,
        key: &str,
        fingerprint: &str,
        processing_ttl_secs: u64,
    ) -> Result<StoredIdempotencyReservation, String>;

    async fn begin_execution(
        &self,
        key: &str,
        fingerprint: &str,
        completed_ttl_secs: u64,
    ) -> Result<(), String>;

    async fn complete(
        &self,
        key: &str,
        fingerprint: &str,
        response: &str,
        completed_ttl_secs: u64,
    ) -> Result<(), String>;

    async fn mark_non_replayable(
        &self,
        key: &str,
        fingerprint: &str,
        completed_ttl_secs: u64,
    ) -> Result<(), String>;

    async fn release(&self, key: &str);
}

impl IdempotencyState {
    pub fn new(store: Option<Arc<dyn HttpIdempotencyStore>>, ttl_seconds: u64) -> Self {
        Self {
            store,
            local: Arc::new(DashMap::new()),
            completed_ttl_secs: ttl_seconds.max(1),
            processing_ttl_secs: DEFAULT_PROCESSING_TTL_SECS,
        }
    }

    pub fn with_processing_ttl(mut self, ttl_seconds: u64) -> Self {
        self.processing_ttl_secs = ttl_seconds.max(1);
        self
    }

    pub async fn reserve(&self, key: &str, fingerprint: &str) -> Result<Reservation, String> {
        if let Some(store) = &self.store {
            if let Some(reservation) = self.local_terminal(key, fingerprint) {
                return Ok(reservation);
            }
            return match store
                .reserve(key, fingerprint, self.processing_ttl_secs)
                .await?
            {
                StoredIdempotencyReservation::Acquired => Ok(Reservation::Acquired),
                StoredIdempotencyReservation::Processing => Ok(Reservation::Processing),
                StoredIdempotencyReservation::Conflict => Ok(Reservation::Conflict),
                StoredIdempotencyReservation::NonReplayable => Ok(Reservation::NonReplayable),
                StoredIdempotencyReservation::Completed(response) => {
                    serde_json::from_str(&response)
                        .map(Reservation::Completed)
                        .map_err(|error| format!("幂等响应缓存无效: {error}"))
                }
            };
        }
        Ok(self.reserve_local(key, fingerprint))
    }

    fn local_terminal(&self, key: &str, fingerprint: &str) -> Option<Reservation> {
        let record = self.local.get(key)?;
        if record.expires_at <= Instant::now() {
            drop(record);
            self.local.remove(key);
            return None;
        }
        if record.fingerprint != fingerprint {
            return Some(Reservation::Conflict);
        }
        match &record.state {
            LocalState::Completed(response) => Some(Reservation::Completed(response.clone())),
            LocalState::NonReplayable => Some(Reservation::NonReplayable),
            LocalState::Processing => None,
        }
    }

    fn reserve_local(&self, key: &str, fingerprint: &str) -> Reservation {
        let now = Instant::now();
        match self.local.entry(key.to_string()) {
            Entry::Vacant(entry) => {
                entry.insert(LocalRecord {
                    fingerprint: fingerprint.to_string(),
                    state: LocalState::Processing,
                    expires_at: now + Duration::from_secs(self.processing_ttl_secs),
                });
                Reservation::Acquired
            }
            Entry::Occupied(mut entry) => {
                if entry.get().expires_at <= now {
                    entry.insert(LocalRecord {
                        fingerprint: fingerprint.to_string(),
                        state: LocalState::Processing,
                        expires_at: now + Duration::from_secs(self.processing_ttl_secs),
                    });
                    return Reservation::Acquired;
                }
                if entry.get().fingerprint != fingerprint {
                    return Reservation::Conflict;
                }
                match &entry.get().state {
                    LocalState::Processing => Reservation::Processing,
                    LocalState::Completed(response) => Reservation::Completed(response.clone()),
                    LocalState::NonReplayable => Reservation::NonReplayable,
                }
            }
        }
    }

    async fn begin_execution(&self, key: &str, fingerprint: &str) -> Result<(), String> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        store
            .begin_execution(key, fingerprint, self.completed_ttl_secs)
            .await
    }

    pub async fn complete(
        &self,
        key: &str,
        fingerprint: &str,
        response: CachedResponse,
    ) -> Result<(), String> {
        if let Some(store) = &self.store {
            let serialized = serde_json::to_string(&response)
                .map_err(|error| format!("无法序列化幂等响应: {error}"))?;
            let result = store
                .complete(key, fingerprint, &serialized, self.completed_ttl_secs)
                .await;
            if result.is_err() {
                self.store_local_terminal(key, fingerprint, LocalState::Completed(response));
            }
            return result;
        }

        self.local.insert(
            key.to_string(),
            LocalRecord {
                fingerprint: fingerprint.to_string(),
                state: LocalState::Completed(response),
                expires_at: Instant::now() + Duration::from_secs(self.completed_ttl_secs),
            },
        );
        Ok(())
    }

    async fn mark_non_replayable(&self, key: &str, fingerprint: &str) -> Result<(), String> {
        if let Some(store) = &self.store {
            let result = store
                .mark_non_replayable(key, fingerprint, self.completed_ttl_secs)
                .await;
            if result.is_err() {
                self.store_local_terminal(key, fingerprint, LocalState::NonReplayable);
            }
            return result;
        }

        self.local.insert(
            key.to_string(),
            LocalRecord {
                fingerprint: fingerprint.to_string(),
                state: LocalState::NonReplayable,
                expires_at: Instant::now() + Duration::from_secs(self.completed_ttl_secs),
            },
        );
        Ok(())
    }

    fn store_local_terminal(&self, key: &str, fingerprint: &str, state: LocalState) {
        self.local.insert(
            key.to_string(),
            LocalRecord {
                fingerprint: fingerprint.to_string(),
                state,
                expires_at: Instant::now() + Duration::from_secs(self.completed_ttl_secs),
            },
        );
    }

    async fn release(&self, key: &str) {
        if let Some(store) = &self.store {
            store.release(key).await;
        } else {
            self.local.remove(key);
        }
    }

    pub fn spawn_gc(&self) {
        let local = Arc::clone(&self.local);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let now = Instant::now();
                local.retain(|_, record| record.expires_at > now);
            }
        });
    }
}

pub async fn idempotency_middleware(
    State(state): State<IdempotencyState>,
    request: Request,
    next: Next,
) -> Response {
    if !is_mutating(request.method()) {
        return next.run(request).await;
    }
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("multipart/form-data"))
        })
    {
        // 上传/导入请求体以流方式传输，通用幂等设施有意不对其缓存或重放。
        return next.run(request).await;
    }

    let Some(raw_key) = request
        .headers()
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
    else {
        return next.run(request).await;
    };
    if raw_key.len() > 128 || raw_key.bytes().any(|byte| !(0x21..=0x7e).contains(&byte)) {
        return (StatusCode::BAD_REQUEST, "invalid Idempotency-Key").into_response();
    }

    let Some(principal) = request
        .extensions()
        .get::<Arc<RequestPrincipal>>()
        .map(Arc::clone)
    else {
        return (StatusCode::UNAUTHORIZED, "authentication required").into_response();
    };
    let method = request.method().clone();
    let request_target = normalized_request_target(&request);

    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, "request body is too large").into_response();
        }
    };
    let fingerprint = request_fingerprint(
        &principal.tenant_id,
        principal.user_id,
        &method,
        &request_target,
        &body,
    );
    let storage_key = storage_key(&principal.tenant_id, principal.user_id, &raw_key);

    match state.reserve(&storage_key, &fingerprint).await {
        Ok(Reservation::Completed(response)) => return rebuild_response(response),
        Ok(Reservation::Processing) => {
            record_idempotency_conflict("processing");
            return conflict_response("an identical request is still processing", 1);
        }
        Ok(Reservation::Conflict) => {
            record_idempotency_conflict("different_fingerprint");
            return (
                StatusCode::CONFLICT,
                "Idempotency-Key was reused with a different request",
            )
                .into_response();
        }
        Ok(Reservation::NonReplayable) => {
            record_idempotency_conflict("non_replayable");
            return (
                StatusCode::CONFLICT,
                "the original result cannot be replayed",
            )
                .into_response();
        }
        Ok(Reservation::Acquired) => {}
        Err(error) => return unavailable_response(error),
    }

    if let Err(error) = state.begin_execution(&storage_key, &fingerprint).await {
        // 即使客户端未收到 Redis 事务响应，它也可能已经设置执行保护。保留任何保护符合失败即拒绝原则：
        // 业务处理器尚未运行，后续请求可在处理/保护 TTL 到期后安全重试，避免产生含义不明的解锁。
        return unavailable_response(error);
    }

    let response = next.run(Request::from_parts(parts, Body::from(body))).await;
    if !response.status().is_success() {
        state.release(&storage_key).await;
        return response;
    }

    let (parts, body) = response.into_parts();
    let body = match to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(error) => {
            // 处理器已返回成功，其副作用可能已经提交。收集响应失败时绝不能释放分布式保护；
            // Redis 可用时将结果标记为不可重放，否则让执行保护自行过期。
            if let Err(mark_error) = state.mark_non_replayable(&storage_key, &fingerprint).await {
                record_redis_degraded("idempotency");
                tracing::error!(error = %mark_error, "failed to protect ambiguous idempotent result");
            }
            tracing::error!(error = %error, "failed to collect idempotent response");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read response body",
            )
                .into_response();
        }
    };

    if body.len() > MAX_CACHED_RESPONSE_BYTES {
        if let Err(error) = state.mark_non_replayable(&storage_key, &fingerprint).await {
            record_redis_degraded("idempotency");
            tracing::error!(error = %error, "failed to mark large idempotent response");
        }
        return Response::from_parts(parts, Body::from(body));
    }

    let cached = CachedResponse {
        status: parts.status.as_u16(),
        body: body.to_vec(),
        headers: cacheable_response_headers(&parts.headers),
    };
    if let Err(error) = state.complete(&storage_key, &fingerprint, cached).await {
        record_redis_degraded("idempotency");
        tracing::error!(error = %error, "failed to persist idempotent response");
    }
    Response::from_parts(parts, Body::from(body))
}
