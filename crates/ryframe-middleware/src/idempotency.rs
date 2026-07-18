use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::{Body, to_bytes},
    extract::{MatchedPath, Request, State},
    http::{
        HeaderMap, HeaderName, HeaderValue, Method, StatusCode,
        header::{self, RETRY_AFTER},
    },
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::{DashMap, mapref::entry::Entry};
use ryframe_auth::RequestPrincipal;
use ryframe_core::RedisClient;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::metrics::{record_idempotency_conflict, record_redis_degraded};

const KEY_PREFIX: &str = "ryframe:v0.5:idempotency:";
const DEFAULT_PROCESSING_TTL_SECS: u64 = 30;
const MAX_REQUEST_BYTES: usize = 10 * 1024 * 1024;
const MAX_CACHED_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct IdempotencyState {
    redis: Option<RedisClient>,
    local: Arc<DashMap<String, LocalRecord>>,
    completed_ttl_secs: u64,
    processing_ttl_secs: u64,
}

#[derive(Clone)]
struct LocalRecord {
    request_hash: String,
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
struct CachedResponse {
    status: u16,
    body: Vec<u8>,
    headers: Vec<CachedHeader>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedHeader {
    name: String,
    value: Vec<u8>,
}

enum Reservation {
    Acquired,
    Processing,
    Conflict,
    Completed(CachedResponse),
    NonReplayable,
}

impl IdempotencyState {
    pub fn new(redis: Option<RedisClient>, ttl_seconds: u64) -> Self {
        Self {
            redis,
            local: Arc::new(DashMap::new()),
            completed_ttl_secs: ttl_seconds.max(1),
            processing_ttl_secs: DEFAULT_PROCESSING_TTL_SECS,
        }
    }

    pub fn with_processing_ttl(mut self, ttl_seconds: u64) -> Self {
        self.processing_ttl_secs = ttl_seconds.max(1);
        self
    }

    async fn reserve(&self, key: &str, request_hash: &str) -> Result<Reservation, String> {
        if let Some(redis) = &self.redis {
            if let Some(reservation) = self.local_terminal(key, request_hash) {
                return Ok(reservation);
            }
            return self.reserve_redis(redis, key, request_hash).await;
        }
        Ok(self.reserve_local(key, request_hash))
    }

    fn local_terminal(&self, key: &str, request_hash: &str) -> Option<Reservation> {
        let record = self.local.get(key)?;
        if record.expires_at <= Instant::now() {
            drop(record);
            self.local.remove(key);
            return None;
        }
        if record.request_hash != request_hash {
            return Some(Reservation::Conflict);
        }
        match &record.state {
            LocalState::Completed(response) => Some(Reservation::Completed(response.clone())),
            LocalState::NonReplayable => Some(Reservation::NonReplayable),
            LocalState::Processing => None,
        }
    }

    async fn reserve_redis(
        &self,
        redis: &RedisClient,
        key: &str,
        request_hash: &str,
    ) -> Result<Reservation, String> {
        let meta_key = meta_key(key);
        let guard_key = guard_key(key);
        let script = r#"
            if redis.call('EXISTS', KEYS[1]) ~= 0 then
                local existing_hash = redis.call('HGET', KEYS[1], 'request_hash')
                if existing_hash ~= ARGV[1] then return 2 end
                local state = redis.call('HGET', KEYS[1], 'state')
                if state == 'processing' then return 3 end
                if state == 'non_replayable' then return 4 end
                if state == 'completed' then return 5 end
                return 6
            end
            if redis.call('EXISTS', KEYS[2]) ~= 0 then
                if redis.call('GET', KEYS[2]) ~= ARGV[1] then return 2 end
                return 4
            end
            redis.call('HSET', KEYS[1], 'state', 'processing', 'request_hash', ARGV[1])
            redis.call('EXPIRE', KEYS[1], tonumber(ARGV[2]))
            return 1
        "#;
        let processing_ttl = self.processing_ttl_secs.to_string();
        let result = redis
            .eval_script(
                script,
                &[meta_key.as_str(), guard_key.as_str()],
                &[request_hash, processing_ttl.as_str()],
            )
            .await
            .map_err(|error| format!("Redis idempotency reservation failed: {error}"))?;

        match result {
            redis::Value::Int(1) => Ok(Reservation::Acquired),
            redis::Value::Int(2) => Ok(Reservation::Conflict),
            redis::Value::Int(3) => Ok(Reservation::Processing),
            redis::Value::Int(4) => Ok(Reservation::NonReplayable),
            redis::Value::Int(5) => {
                let response = redis
                    .get(response_key(key))
                    .await
                    .map_err(|error| format!("Redis idempotency response read failed: {error}"))?
                    .ok_or_else(|| "completed idempotency response is missing".to_string())?;
                serde_json::from_str(&response)
                    .map(Reservation::Completed)
                    .map_err(|error| format!("invalid cached idempotency response: {error}"))
            }
            value => Err(format!("unexpected Redis idempotency result: {value:?}")),
        }
    }

    fn reserve_local(&self, key: &str, request_hash: &str) -> Reservation {
        let now = Instant::now();
        match self.local.entry(key.to_string()) {
            Entry::Vacant(entry) => {
                entry.insert(LocalRecord {
                    request_hash: request_hash.to_string(),
                    state: LocalState::Processing,
                    expires_at: now + Duration::from_secs(self.processing_ttl_secs),
                });
                Reservation::Acquired
            }
            Entry::Occupied(mut entry) => {
                if entry.get().expires_at <= now {
                    entry.insert(LocalRecord {
                        request_hash: request_hash.to_string(),
                        state: LocalState::Processing,
                        expires_at: now + Duration::from_secs(self.processing_ttl_secs),
                    });
                    return Reservation::Acquired;
                }
                if entry.get().request_hash != request_hash {
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

    async fn begin_execution(&self, key: &str, request_hash: &str) -> Result<(), String> {
        let Some(redis) = &self.redis else {
            return Ok(());
        };
        let meta_key = meta_key(key);
        let guard_key = guard_key(key);
        let script = r#"
            if redis.call('HGET', KEYS[1], 'request_hash') ~= ARGV[1] then return 0 end
            if redis.call('HGET', KEYS[1], 'state') ~= 'processing' then return 0 end
            redis.call('SETEX', KEYS[2], tonumber(ARGV[2]), ARGV[1])
            return 1
        "#;
        let ttl = self.completed_ttl_secs.to_string();
        match redis
            .eval_script(
                script,
                &[meta_key.as_str(), guard_key.as_str()],
                &[request_hash, ttl.as_str()],
            )
            .await
        {
            Ok(redis::Value::Int(1)) => Ok(()),
            Ok(value) => Err(format!(
                "idempotency execution guard was rejected: {value:?}"
            )),
            Err(error) => Err(format!("Redis idempotency execution guard failed: {error}")),
        }
    }

    async fn complete(
        &self,
        key: &str,
        request_hash: &str,
        response: CachedResponse,
    ) -> Result<(), String> {
        if let Some(redis) = &self.redis {
            let serialized = serde_json::to_string(&response)
                .map_err(|error| format!("cannot serialize idempotency response: {error}"))?;
            let meta_key = meta_key(key);
            let response_key = response_key(key);
            let guard_key = guard_key(key);
            let script = r#"
                if redis.call('HGET', KEYS[1], 'request_hash') ~= ARGV[1] then return 0 end
                redis.call('SETEX', KEYS[2], tonumber(ARGV[2]), ARGV[3])
                redis.call('HSET', KEYS[1], 'state', 'completed')
                redis.call('EXPIRE', KEYS[1], tonumber(ARGV[2]))
                redis.call('DEL', KEYS[3])
                return 1
            "#;
            let ttl = self.completed_ttl_secs.to_string();
            let result = match redis
                .eval_script(
                    script,
                    &[meta_key.as_str(), response_key.as_str(), guard_key.as_str()],
                    &[request_hash, ttl.as_str(), serialized.as_str()],
                )
                .await
            {
                Ok(redis::Value::Int(1)) => Ok(()),
                Ok(value) => Err(format!("idempotency completion was rejected: {value:?}")),
                Err(error) => Err(format!("Redis idempotency completion failed: {error}")),
            };
            if result.is_err() {
                self.store_local_terminal(key, request_hash, LocalState::Completed(response));
            }
            return result;
        }

        self.local.insert(
            key.to_string(),
            LocalRecord {
                request_hash: request_hash.to_string(),
                state: LocalState::Completed(response),
                expires_at: Instant::now() + Duration::from_secs(self.completed_ttl_secs),
            },
        );
        Ok(())
    }

    async fn mark_non_replayable(&self, key: &str, request_hash: &str) -> Result<(), String> {
        if let Some(redis) = &self.redis {
            let meta_key = meta_key(key);
            let script = r#"
                if redis.call('HGET', KEYS[1], 'request_hash') ~= ARGV[1] then return 0 end
                redis.call('HSET', KEYS[1], 'state', 'non_replayable')
                redis.call('EXPIRE', KEYS[1], tonumber(ARGV[2]))
                return 1
            "#;
            let ttl = self.completed_ttl_secs.to_string();
            let result = match redis
                .eval_script(script, &[meta_key.as_str()], &[request_hash, ttl.as_str()])
                .await
            {
                Ok(redis::Value::Int(1)) => Ok(()),
                Ok(value) => Err(format!("non-replayable marker was rejected: {value:?}")),
                Err(error) => Err(format!("Redis idempotency marker failed: {error}")),
            };
            if result.is_err() {
                self.store_local_terminal(key, request_hash, LocalState::NonReplayable);
            }
            return result;
        }

        self.local.insert(
            key.to_string(),
            LocalRecord {
                request_hash: request_hash.to_string(),
                state: LocalState::NonReplayable,
                expires_at: Instant::now() + Duration::from_secs(self.completed_ttl_secs),
            },
        );
        Ok(())
    }

    fn store_local_terminal(&self, key: &str, request_hash: &str, state: LocalState) {
        self.local.insert(
            key.to_string(),
            LocalRecord {
                request_hash: request_hash.to_string(),
                state,
                expires_at: Instant::now() + Duration::from_secs(self.completed_ttl_secs),
            },
        );
    }

    async fn release(&self, key: &str) {
        if let Some(redis) = &self.redis {
            let _ = redis.del(meta_key(key)).await;
            let _ = redis.del(response_key(key)).await;
            let _ = redis.del(guard_key(key)).await;
        } else {
            self.local.remove(key);
        }
    }

    pub fn spawn_gc(&self) {
        let local = self.local.clone();
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
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("multipart/form-data"))
        })
    {
        // Upload/import bodies are streamed and intentionally never cached or
        // replayed by the generic idempotency facility.
        return next.run(request).await;
    }

    let Some(raw_key) = request
        .headers()
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
    else {
        return next.run(request).await;
    };
    if raw_key.len() > 128 || raw_key.bytes().any(|byte| !(0x21..=0x7e).contains(&byte)) {
        return (StatusCode::BAD_REQUEST, "invalid Idempotency-Key").into_response();
    }

    let Some(principal) = request.extensions().get::<RequestPrincipal>().cloned() else {
        return (StatusCode::UNAUTHORIZED, "authentication required").into_response();
    };
    let method = request.method().clone();
    let path = normalized_request_path(&request);

    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, "request body is too large").into_response();
        }
    };
    let request_hash = hex_sha256(&body);
    let scoped_key = scoped_key(
        &principal.tenant_id,
        principal.user_id,
        &method,
        &path,
        &raw_key,
    );

    match state.reserve(&scoped_key, &request_hash).await {
        Ok(Reservation::Completed(response)) => return rebuild_response(&response),
        Ok(Reservation::Processing) => {
            record_idempotency_conflict("processing");
            return conflict_response("an identical request is still processing", 1);
        }
        Ok(Reservation::Conflict) => {
            record_idempotency_conflict("different_body");
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

    if let Err(error) = state.begin_execution(&scoped_key, &request_hash).await {
        // The Lua command may have installed the execution guard even when the
        // client did not receive its reply. Keeping any guard is fail-closed:
        // no business handler has run yet, and a later request can safely retry
        // after the processing/guard TTL instead of risking an ambiguous unlock.
        return unavailable_response(error);
    }

    let response = next.run(Request::from_parts(parts, Body::from(body))).await;
    if !response.status().is_success() {
        state.release(&scoped_key).await;
        return response;
    }

    let (parts, body) = response.into_parts();
    let body = match to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(error) => {
            // The handler has already returned success, so its side effect may
            // be committed. Never release the distributed guard on a response
            // collection failure; mark the outcome non-replayable when Redis is
            // available and otherwise leave the execution guard to expire.
            if let Err(mark_error) = state.mark_non_replayable(&scoped_key, &request_hash).await {
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
        if let Err(error) = state.mark_non_replayable(&scoped_key, &request_hash).await {
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
    if let Err(error) = state.complete(&scoped_key, &request_hash, cached).await {
        record_redis_degraded("idempotency");
        tracing::error!(error = %error, "failed to persist idempotent response");
    }
    Response::from_parts(parts, Body::from(body))
}

fn is_mutating(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

fn normalized_request_path(request: &Request) -> String {
    request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_else(|| normalize_unmatched_path(request.uri().path()))
}

/// Preserve the routing semantics of an unmatched path while canonicalizing
/// percent-escape casing. In particular, this intentionally does not collapse
/// slashes, remove trailing slashes, or decode reserved characters.
fn normalize_unmatched_path(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }

    let mut normalized = path.as_bytes().to_vec();
    let mut index = 0;
    while index < normalized.len() {
        if normalized[index] == b'%'
            && index + 2 < normalized.len()
            && normalized[index + 1].is_ascii_hexdigit()
            && normalized[index + 2].is_ascii_hexdigit()
        {
            normalized[index + 1] = normalized[index + 1].to_ascii_uppercase();
            normalized[index + 2] = normalized[index + 2].to_ascii_uppercase();
            index += 3;
        } else {
            index += 1;
        }
    }
    String::from_utf8(normalized).unwrap_or_else(|_| path.to_owned())
}

fn is_replayable_response_header(name: &HeaderName) -> bool {
    matches!(name.as_str(), "content-type" | "location" | "etag")
}

fn cacheable_response_headers(headers: &HeaderMap) -> Vec<CachedHeader> {
    let mut cached = Vec::new();
    for name in [header::CONTENT_TYPE, header::LOCATION, header::ETAG] {
        for value in headers.get_all(&name) {
            cached.push(CachedHeader {
                name: name.as_str().to_owned(),
                value: value.as_bytes().to_vec(),
            });
        }
    }
    cached
}

fn scoped_key(tenant_id: &str, user_id: i64, method: &Method, path: &str, raw_key: &str) -> String {
    hex_sha256(format!("{tenant_id}\n{user_id}\n{method}\n{path}\n{raw_key}").as_bytes())
}

fn hex_sha256(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn meta_key(key: &str) -> String {
    format!("{KEY_PREFIX}{key}:meta")
}

fn response_key(key: &str) -> String {
    format!("{KEY_PREFIX}{key}:response")
}

fn guard_key(key: &str) -> String {
    format!("{KEY_PREFIX}{key}:guard")
}

fn rebuild_response(cached: &CachedResponse) -> Response {
    let status = StatusCode::from_u16(cached.status).unwrap_or(StatusCode::OK);
    let mut response = Response::new(Body::from(cached.body.clone()));
    *response.status_mut() = status;
    for cached_header in &cached.headers {
        let Ok(name) = HeaderName::from_bytes(cached_header.name.as_bytes()) else {
            continue;
        };
        if !is_replayable_response_header(&name) {
            continue;
        }
        if let Ok(value) = HeaderValue::from_bytes(&cached_header.value) {
            response.headers_mut().append(name, value);
        }
    }
    response
        .headers_mut()
        .insert("X-Idempotency-Replay", HeaderValue::from_static("true"));
    response
}

fn conflict_response(message: &str, retry_after_secs: u64) -> Response {
    let mut response = (StatusCode::CONFLICT, message.to_string()).into_response();
    response.headers_mut().insert(
        RETRY_AFTER,
        HeaderValue::from_str(&retry_after_secs.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("1")),
    );
    response
}

fn unavailable_response(error: String) -> Response {
    record_redis_degraded("idempotency");
    tracing::error!(error = %error, "idempotency backend unavailable");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "idempotency service unavailable",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, header};

    use super::{
        CachedResponse, cacheable_response_headers, normalize_unmatched_path, rebuild_response,
    };

    #[test]
    fn unmatched_path_normalization_is_stable_and_semantics_preserving() {
        assert_eq!(normalize_unmatched_path(""), "/");
        assert_eq!(
            normalize_unmatched_path("/files//%7e/%2f/"),
            "/files//%7E/%2F/"
        );
        assert_eq!(normalize_unmatched_path("/files/%zz"), "/files/%zz");
        assert_eq!(
            normalize_unmatched_path("/文件/%e4%b8%ad"),
            "/文件/%E4%B8%AD"
        );
    }

    #[test]
    fn redis_response_payload_round_trip_excludes_sensitive_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        headers.insert(header::LOCATION, "/resources/42".parse().unwrap());
        headers.insert(header::ETAG, "\"v1\"".parse().unwrap());
        headers.insert(header::SET_COOKIE, "session=secret".parse().unwrap());
        headers.insert(header::AUTHORIZATION, "Bearer secret".parse().unwrap());

        let cached = CachedResponse {
            status: 201,
            body: br#"{"ok":true}"#.to_vec(),
            headers: cacheable_response_headers(&headers),
        };
        let serialized = serde_json::to_string(&cached).unwrap();
        assert!(!serialized.contains("set-cookie"));
        assert!(!serialized.contains("authorization"));

        let decoded: CachedResponse = serde_json::from_str(&serialized).unwrap();
        let replay = rebuild_response(&decoded);
        assert_eq!(replay.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(replay.headers()[header::LOCATION], "/resources/42");
        assert_eq!(replay.headers()[header::ETAG], "\"v1\"");
        assert!(!replay.headers().contains_key(header::SET_COOKIE));
        assert!(!replay.headers().contains_key(header::AUTHORIZATION));
    }
}
