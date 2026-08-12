//! 刷新令牌族状态。
//!
//! 一个令牌族由稳定会话标识（`sid`）定位。Redis 模式使用一个 Lua 脚本进行比较并交换轮换，
//! 以确保两个应用实例无法同时接受同一个刷新令牌。

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use dashmap::DashMap;
use ryframe_kernel::{AppError, AppResult};

use crate::RedisClient;

const KEY_PREFIX: &str = "ryframe:v0.5:refresh-family:";
const TENANT_INDEX_PREFIX: &str = "ryframe:v0.5:refresh-family-index:tenant:";
const TENANT_USER_INDEX_PREFIX: &str = "ryframe:v0.5:refresh-family-index:tenant-user:";
const CONCURRENT_GRACE_SECONDS: i64 = 5;
const MAX_BULK_SESSION_CANDIDATES: usize = 256;
static LOCAL_FAMILIES: OnceLock<Arc<DashMap<String, RefreshFamily>>> = OnceLock::new();
static LOCAL_MUTATION_LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();

const ROTATE_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return {0, '', 0} end
if redis.call('HGET', KEYS[1], 'revoked') == '1' then return {0, '', 0} end
local current = redis.call('HGET', KEYS[1], 'current_jti')
local previous = redis.call('HGET', KEYS[1], 'previous_jti')
local last_attempt = redis.call('HGET', KEYS[1], 'last_attempt_id')
local rotated_at = tonumber(redis.call('HGET', KEYS[1], 'rotated_at') or '0')
local absolute_exp = tonumber(redis.call('HGET', KEYS[1], 'absolute_exp') or '0')
local now = tonumber(ARGV[3])
if absolute_exp <= now then
  local tenant_id = redis.call('HGET', KEYS[1], 'tenant_id')
  local user_id = redis.call('HGET', KEYS[1], 'user_id')
  local sid = redis.call('HGET', KEYS[1], 'sid')
  if tenant_id and user_id and sid then
    local tenant_key = ARGV[6] .. tenant_id
    local user_key = ARGV[7] .. tenant_id .. ':' .. user_id
    redis.call('SREM', tenant_key, sid)
    redis.call('SREM', user_key, sid)
    if redis.call('SCARD', tenant_key) == 0 then redis.call('DEL', tenant_key) end
    if redis.call('SCARD', user_key) == 0 then redis.call('DEL', user_key) end
  end
  redis.call('DEL', KEYS[1])
  return {0, '', 0}
end
if current == ARGV[1] then
  redis.call('HSET', KEYS[1],
    'previous_jti', current,
    'current_jti', ARGV[2],
    'rotated_at', ARGV[3],
    'last_attempt_id', ARGV[5])
  redis.call('EXPIREAT', KEYS[1], absolute_exp)
  return {1, ARGV[2], now}
end
if previous == ARGV[1] then
  if last_attempt == ARGV[5] then
    return {4, current, rotated_at}
  end
  if now - rotated_at <= tonumber(ARGV[4]) then
    return {2, '', 0}
  end
end
redis.call('HSET', KEYS[1], 'revoked', '1')
redis.call('EXPIREAT', KEYS[1], absolute_exp)
local tenant_id = redis.call('HGET', KEYS[1], 'tenant_id')
local user_id = redis.call('HGET', KEYS[1], 'user_id')
local sid = redis.call('HGET', KEYS[1], 'sid')
if tenant_id and user_id and sid then
  local tenant_key = ARGV[6] .. tenant_id
  local user_key = ARGV[7] .. tenant_id .. ':' .. user_id
  redis.call('SREM', tenant_key, sid)
  redis.call('SREM', user_key, sid)
  if redis.call('SCARD', tenant_key) == 0 then redis.call('DEL', tenant_key) end
  if redis.call('SCARD', user_key) == 0 then redis.call('DEL', user_key) end
end
return {3, '', 0}
"#;

const REGISTER_SCRIPT: &str = r#"
local new_family_key = KEYS[1]
local new_tenant_key = ARGV[10] .. ARGV[2]
local new_user_key = ARGV[11] .. ARGV[2] .. ':' .. ARGV[3]
local new_family_type = redis.call('TYPE', new_family_key)['ok']
local new_tenant_type = redis.call('TYPE', new_tenant_key)['ok']
local new_user_type = redis.call('TYPE', new_user_key)['ok']
if new_family_type ~= 'none' and new_family_type ~= 'hash' then
  return redis.error_reply('invalid refresh family type')
end
if new_tenant_type ~= 'none' and new_tenant_type ~= 'set' then
  return redis.error_reply('invalid tenant session index type')
end
if new_user_type ~= 'none' and new_user_type ~= 'set' then
  return redis.error_reply('invalid user session index type')
end
-- 首次注册时 rotated_at 就是本次签发时间，可作为有界索引清理的当前时间基准。
local now = tonumber(ARGV[6])
if not now or not tonumber(ARGV[7]) then
  return redis.error_reply('invalid refresh family expiry')
end
local indexed_sids = redis.call('SMEMBERS', new_user_key)
if #indexed_sids > 256 then
  return 2
end
local stale_sids = {}
local active_count = 0
local new_sid_indexed = false
for _, indexed_sid in ipairs(indexed_sids) do
  local indexed_family_key = ARGV[12] .. indexed_sid
  local indexed_type = redis.call('TYPE', indexed_family_key)['ok']
  if indexed_type ~= 'none' and indexed_type ~= 'hash' then
    return redis.error_reply('invalid indexed refresh family type')
  end
  if indexed_type == 'none' then
    table.insert(stale_sids, indexed_sid)
  else
    local indexed_tenant = redis.call('HGET', indexed_family_key, 'tenant_id')
    local indexed_user = redis.call('HGET', indexed_family_key, 'user_id')
    local indexed_exp_raw = redis.call('HGET', indexed_family_key, 'absolute_exp')
    local indexed_exp = tonumber(indexed_exp_raw or '')
    local indexed_revoked = redis.call('HGET', indexed_family_key, 'revoked')
    if not indexed_exp or (indexed_revoked ~= '0' and indexed_revoked ~= '1') then
      return redis.error_reply('invalid indexed refresh family fields')
    end
    if indexed_tenant == ARGV[2] and indexed_user == ARGV[3]
      and indexed_exp > now and indexed_revoked == '0' then
      active_count = active_count + 1
      if indexed_sid == ARGV[1] then new_sid_indexed = true end
    else
      table.insert(stale_sids, indexed_sid)
    end
  end
end
if ARGV[8] == '0' and not new_sid_indexed and active_count >= 256 then
  return 2
end
local old_tenant_id = redis.call('HGET', KEYS[1], 'tenant_id')
local old_user_id = redis.call('HGET', KEYS[1], 'user_id')
local old_tenant_key = false
local old_user_key = false
if old_tenant_id and old_user_id then
  old_tenant_key = ARGV[10] .. old_tenant_id
  old_user_key = ARGV[11] .. old_tenant_id .. ':' .. old_user_id
  local old_tenant_type = redis.call('TYPE', old_tenant_key)['ok']
  local old_user_type = redis.call('TYPE', old_user_key)['ok']
  if old_tenant_type ~= 'none' and old_tenant_type ~= 'set' then
    return redis.error_reply('invalid old tenant session index type')
  end
  if old_user_type ~= 'none' and old_user_type ~= 'set' then
    return redis.error_reply('invalid old user session index type')
  end
end
if old_tenant_key and old_user_key then
  redis.call('SREM', old_tenant_key, ARGV[1])
  redis.call('SREM', old_user_key, ARGV[1])
  if redis.call('SCARD', old_tenant_key) == 0 then redis.call('DEL', old_tenant_key) end
  if redis.call('SCARD', old_user_key) == 0 then redis.call('DEL', old_user_key) end
end
for _, stale_sid in ipairs(stale_sids) do
  redis.call('SREM', new_tenant_key, stale_sid)
  redis.call('SREM', new_user_key, stale_sid)
end
redis.call('HSET', KEYS[1],
  'sid', ARGV[1],
  'tenant_id', ARGV[2],
  'user_id', ARGV[3],
  'current_jti', ARGV[4],
  'previous_jti', ARGV[5],
  'rotated_at', ARGV[6],
  'absolute_exp', ARGV[7],
  'revoked', ARGV[8],
  'last_attempt_id', ARGV[9])
redis.call('EXPIREAT', KEYS[1], tonumber(ARGV[7]))
if ARGV[8] == '0' then
  local tenant_key = ARGV[10] .. ARGV[2]
  local user_key = ARGV[11] .. ARGV[2] .. ':' .. ARGV[3]
  redis.call('SADD', tenant_key, ARGV[1])
  redis.call('SADD', user_key, ARGV[1])
  local tenant_expiry = redis.call('EXPIRETIME', tenant_key)
  local user_expiry = redis.call('EXPIRETIME', user_key)
  if tenant_expiry < tonumber(ARGV[7]) then
    redis.call('EXPIREAT', tenant_key, tonumber(ARGV[7]))
  end
  if user_expiry < tonumber(ARGV[7]) then
    redis.call('EXPIREAT', user_key, tonumber(ARGV[7]))
  end
end
return 1
"#;

const REVOKE_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return 0 end
local tenant_id = redis.call('HGET', KEYS[1], 'tenant_id')
local user_id = redis.call('HGET', KEYS[1], 'user_id')
local sid = redis.call('HGET', KEYS[1], 'sid')
local absolute_exp = tonumber(redis.call('HGET', KEYS[1], 'absolute_exp') or '0')
if absolute_exp <= tonumber(ARGV[1]) then
  redis.call('DEL', KEYS[1])
else
  redis.call('HSET', KEYS[1], 'revoked', '1')
end
if tenant_id and user_id and sid then
  local tenant_key = ARGV[2] .. tenant_id
  local user_key = ARGV[3] .. tenant_id .. ':' .. user_id
  redis.call('SREM', tenant_key, sid)
  redis.call('SREM', user_key, sid)
  if redis.call('SCARD', tenant_key) == 0 then redis.call('DEL', tenant_key) end
  if redis.call('SCARD', user_key) == 0 then redis.call('DEL', user_key) end
end
if absolute_exp <= tonumber(ARGV[1]) then return 0 end
return 1
"#;

// 刷新令牌族是强制登出的权威依据。租户和用户校验、撤销、索引清理必须位于同一个脚本中，
// 防止展示索引被利用来撤销其他身份的会话。
const REVOKE_FOR_TENANT_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return 0 end
if redis.call('HGET', KEYS[1], 'tenant_id') ~= ARGV[1] then return 0 end
local user_id = redis.call('HGET', KEYS[1], 'user_id')
local sid = redis.call('HGET', KEYS[1], 'sid')
local absolute_exp = tonumber(redis.call('HGET', KEYS[1], 'absolute_exp') or '0')
if absolute_exp <= tonumber(ARGV[2]) then
  redis.call('DEL', KEYS[1])
else
  redis.call('HSET', KEYS[1], 'revoked', '1')
end
local tenant_key = ARGV[3] .. ARGV[1]
local user_key = ARGV[4] .. ARGV[1] .. ':' .. user_id
redis.call('SREM', tenant_key, sid)
redis.call('SREM', user_key, sid)
if redis.call('SCARD', tenant_key) == 0 then redis.call('DEL', tenant_key) end
if redis.call('SCARD', user_key) == 0 then redis.call('DEL', user_key) end
if absolute_exp <= tonumber(ARGV[2]) then return 0 end
return 1
"#;

const REVOKE_FOR_USER_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return 0 end
if redis.call('HGET', KEYS[1], 'tenant_id') ~= ARGV[1] then return 0 end
if redis.call('HGET', KEYS[1], 'user_id') ~= ARGV[2] then return 0 end
local sid = redis.call('HGET', KEYS[1], 'sid')
local absolute_exp = tonumber(redis.call('HGET', KEYS[1], 'absolute_exp') or '0')
local tenant_key = ARGV[4] .. ARGV[1]
local user_key = ARGV[5] .. ARGV[1] .. ':' .. ARGV[2]
if absolute_exp <= tonumber(ARGV[3]) then
  redis.call('DEL', KEYS[1])
  redis.call('SREM', tenant_key, sid)
  redis.call('SREM', user_key, sid)
  if redis.call('SCARD', tenant_key) == 0 then redis.call('DEL', tenant_key) end
  if redis.call('SCARD', user_key) == 0 then redis.call('DEL', user_key) end
  return 0
end
local already_revoked = redis.call('HGET', KEYS[1], 'revoked') == '1'
redis.call('HSET', KEYS[1], 'revoked', '1')
redis.call('SREM', tenant_key, sid)
redis.call('SREM', user_key, sid)
if redis.call('SCARD', tenant_key) == 0 then redis.call('DEL', tenant_key) end
if redis.call('SCARD', user_key) == 0 then redis.call('DEL', user_key) end
if already_revoked then return 2 end
return 1
"#;

const REVOKE_OTHER_SESSIONS_FOR_USER_SCRIPT: &str = r#"
local tenant_id = ARGV[1]
local user_id = ARGV[2]
local current_sid = ARGV[3]
local now = tonumber(ARGV[4])
local family_prefix = ARGV[5]
local tenant_index = ARGV[6] .. tenant_id
local user_index = ARGV[7] .. tenant_id .. ':' .. user_id
local tenant_index_type = redis.call('TYPE', tenant_index)['ok']
local user_index_type = redis.call('TYPE', user_index)['ok']
if tenant_index_type ~= 'none' and tenant_index_type ~= 'set' then
  return redis.error_reply('invalid tenant session index type')
end
if user_index_type ~= 'none' and user_index_type ~= 'set' then
  return redis.error_reply('invalid user session index type')
end
local candidates = {}
local seen = {}
for index = 8, #ARGV do
  local candidate_sid = ARGV[index]
  if candidate_sid ~= current_sid and not seen[candidate_sid] then
    seen[candidate_sid] = true
    table.insert(candidates, candidate_sid)
  end
end
for _, indexed_sid in ipairs(redis.call('SMEMBERS', user_index)) do
  if indexed_sid ~= current_sid and not seen[indexed_sid] then
    seen[indexed_sid] = true
    table.insert(candidates, indexed_sid)
  end
end
if #candidates > 256 then
  return -1
end
for _, sid in ipairs(candidates) do
  if sid ~= current_sid then
    local family_key = family_prefix .. sid
    local family_type = redis.call('TYPE', family_key)['ok']
    if family_type ~= 'none' and family_type ~= 'hash' then
      return redis.error_reply('invalid refresh family type')
    end
    if family_type == 'hash' then
      local family_tenant = redis.call('HGET', family_key, 'tenant_id')
      local family_user = redis.call('HGET', family_key, 'user_id')
      local absolute_exp_raw = redis.call('HGET', family_key, 'absolute_exp')
      local absolute_exp = tonumber(absolute_exp_raw or '')
      local revoked = redis.call('HGET', family_key, 'revoked')
      if not family_tenant or not family_user or not absolute_exp
        or (revoked ~= '0' and revoked ~= '1') then
        return redis.error_reply('invalid refresh family fields')
      end
    end
  end
end
local revoked_count = 0
for _, sid in ipairs(candidates) do
  local family_key = family_prefix .. sid
  if redis.call('TYPE', family_key)['ok'] == 'hash' then
    local family_tenant = redis.call('HGET', family_key, 'tenant_id')
    local family_user = redis.call('HGET', family_key, 'user_id')
    local absolute_exp = tonumber(redis.call('HGET', family_key, 'absolute_exp') or '0')
    local revoked = redis.call('HGET', family_key, 'revoked') == '1'
    if family_tenant == tenant_id and family_user == user_id then
      if absolute_exp <= now then
        redis.call('DEL', family_key)
        redis.call('SREM', tenant_index, sid)
        redis.call('SREM', user_index, sid)
      elseif not revoked then
        redis.call('HSET', family_key, 'revoked', '1')
        redis.call('EXPIREAT', family_key, absolute_exp)
        redis.call('SREM', tenant_index, sid)
        redis.call('SREM', user_index, sid)
        revoked_count = revoked_count + 1
      else
        redis.call('SREM', tenant_index, sid)
        redis.call('SREM', user_index, sid)
    end
  end
end
end
if redis.call('SCARD', tenant_index) == 0 then redis.call('DEL', tenant_index) end
if redis.call('SCARD', user_index) == 0 then redis.call('DEL', user_index) end
return revoked_count
"#;

const ACTIVE_IDENTITY_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return {} end
local sid = redis.call('HGET', KEYS[1], 'sid')
local tenant_id = redis.call('HGET', KEYS[1], 'tenant_id')
local user_id = redis.call('HGET', KEYS[1], 'user_id')
local absolute_exp = tonumber(redis.call('HGET', KEYS[1], 'absolute_exp') or '0')
local revoked = redis.call('HGET', KEYS[1], 'revoked') == '1'
if revoked or absolute_exp <= tonumber(ARGV[1]) then
  if tenant_id and user_id and sid then
    local tenant_key = ARGV[2] .. tenant_id
    local user_key = ARGV[3] .. tenant_id .. ':' .. user_id
    redis.call('SREM', tenant_key, sid)
    redis.call('SREM', user_key, sid)
    if redis.call('SCARD', tenant_key) == 0 then redis.call('DEL', tenant_key) end
    if redis.call('SCARD', user_key) == 0 then redis.call('DEL', user_key) end
  end
  if absolute_exp <= tonumber(ARGV[1]) then redis.call('DEL', KEYS[1]) end
  return {}
end
return {tenant_id, user_id, tostring(absolute_exp)}
"#;

const IS_ACTIVE_FOR_IDENTITY_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return 0 end
local sid = redis.call('HGET', KEYS[1], 'sid')
local tenant_id = redis.call('HGET', KEYS[1], 'tenant_id')
local user_id = redis.call('HGET', KEYS[1], 'user_id')
local absolute_exp = tonumber(redis.call('HGET', KEYS[1], 'absolute_exp') or '0')
local revoked = redis.call('HGET', KEYS[1], 'revoked') == '1'
local active = not revoked
  and absolute_exp > tonumber(ARGV[3])
  and tenant_id == ARGV[1]
  and user_id == ARGV[2]
if active then return 1 end
if tenant_id and user_id and sid and (revoked or absolute_exp <= tonumber(ARGV[3])) then
  local tenant_key = ARGV[4] .. tenant_id
  local user_key = ARGV[5] .. tenant_id .. ':' .. user_id
  redis.call('SREM', tenant_key, sid)
  redis.call('SREM', user_key, sid)
  if redis.call('SCARD', tenant_key) == 0 then redis.call('DEL', tenant_key) end
  if redis.call('SCARD', user_key) == 0 then redis.call('DEL', user_key) end
end
if absolute_exp <= tonumber(ARGV[3]) then redis.call('DEL', KEYS[1]) end
return 0
"#;

const USER_SESSION_SIDS_SCRIPT: &str = r#"
local result = {}
local sids = redis.call('SMEMBERS', KEYS[1])
for _, sid in ipairs(sids) do
  local family_key = ARGV[3] .. sid
  local tenant_id = redis.call('HGET', family_key, 'tenant_id')
  local user_id = redis.call('HGET', family_key, 'user_id')
  local absolute_exp = tonumber(redis.call('HGET', family_key, 'absolute_exp') or '0')
  local active = tenant_id == ARGV[1]
    and user_id == ARGV[2]
    and redis.call('HGET', family_key, 'revoked') ~= '1'
    and absolute_exp > tonumber(ARGV[4])
  if active then
    table.insert(result, sid)
  else
    redis.call('SREM', KEYS[1], sid)
    if not tenant_id or tenant_id ~= ARGV[1] or absolute_exp <= tonumber(ARGV[4])
      or redis.call('HGET', family_key, 'revoked') == '1' then
      redis.call('SREM', KEYS[2], sid)
    end
    if absolute_exp > 0 and absolute_exp <= tonumber(ARGV[4]) then
      redis.call('DEL', family_key)
    end
  end
end
if redis.call('SCARD', KEYS[1]) == 0 then redis.call('DEL', KEYS[1]) end
if redis.call('SCARD', KEYS[2]) == 0 then redis.call('DEL', KEYS[2]) end
return result
"#;

const TENANT_SESSION_SIDS_SCRIPT: &str = r#"
local result = {}
local sids = redis.call('SMEMBERS', KEYS[1])
for _, sid in ipairs(sids) do
  local family_key = ARGV[2] .. sid
  local tenant_id = redis.call('HGET', family_key, 'tenant_id')
  local user_id = redis.call('HGET', family_key, 'user_id')
  local absolute_exp = tonumber(redis.call('HGET', family_key, 'absolute_exp') or '0')
  local active = tenant_id == ARGV[1]
    and user_id
    and redis.call('HGET', family_key, 'revoked') ~= '1'
    and absolute_exp > tonumber(ARGV[3])
  if active then
    table.insert(result, sid)
  else
    redis.call('SREM', KEYS[1], sid)
    if tenant_id and user_id then
      local user_key = ARGV[4] .. tenant_id .. ':' .. user_id
      redis.call('SREM', user_key, sid)
      if redis.call('SCARD', user_key) == 0 then redis.call('DEL', user_key) end
    end
    if absolute_exp > 0 and absolute_exp <= tonumber(ARGV[3]) then
      redis.call('DEL', family_key)
    end
  end
end
if redis.call('SCARD', KEYS[1]) == 0 then redis.call('DEL', KEYS[1]) end
return result
"#;

#[derive(Debug, Clone)]
pub struct RefreshFamily {
    pub sid: String,
    pub tenant_id: String,
    pub user_id: i64,
    pub current_jti: String,
    pub previous_jti: Option<String>,
    pub last_attempt_id: Option<String>,
    pub rotated_at: i64,
    pub absolute_exp: i64,
    pub revoked: bool,
}

/// 活跃刷新会话的稳定身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshSessionIdentity {
    pub tenant_id: String,
    pub user_id: i64,
    pub absolute_exp: i64,
}

/// 按租户和用户撤销会话的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshSessionRevocation {
    Revoked,
    AlreadyRevoked,
    NotFoundOrForeign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshRotation {
    Rotated { current_jti: String, issued_at: i64 },
    Recovered { current_jti: String, issued_at: i64 },
    Concurrent,
    Replayed,
    MissingOrRevoked,
}

#[derive(Clone)]
pub struct RefreshSessionStore {
    redis: Option<RedisClient>,
    local: Arc<DashMap<String, RefreshFamily>>,
    local_mutation_lock: Arc<Mutex<()>>,
}

impl RefreshSessionStore {
    pub fn new(redis: Option<RedisClient>) -> Self {
        Self {
            redis,
            local: LOCAL_FAMILIES
                .get_or_init(|| Arc::new(DashMap::new()))
                .clone(),
            local_mutation_lock: LOCAL_MUTATION_LOCK
                .get_or_init(|| Arc::new(Mutex::new(())))
                .clone(),
        }
    }

    fn lock_local_mutations(&self) -> AppResult<MutexGuard<'_, ()>> {
        self.local_mutation_lock.lock().map_err(|error| {
            tracing::error!(%error, "本地会话变更锁已损坏");
            AppError::ServiceUnavailable("session service unavailable".into())
        })
    }

    pub fn is_distributed(&self) -> bool {
        self.redis.is_some()
    }

    pub async fn register(&self, family: RefreshFamily) -> AppResult<()> {
        remaining_ttl(family.absolute_exp)?;
        if let Some(redis) = &self.redis {
            let key = family_key(&family.sid);
            let previous = family.previous_jti.as_deref().unwrap_or("");
            let user_id = family.user_id.to_string();
            let rotated_at = family.rotated_at.to_string();
            let absolute_exp = family.absolute_exp.to_string();
            let result = redis
                .eval_script(
                    REGISTER_SCRIPT,
                    &[key.as_str()],
                    &[
                        family.sid.as_str(),
                        family.tenant_id.as_str(),
                        user_id.as_str(),
                        family.current_jti.as_str(),
                        previous,
                        rotated_at.as_str(),
                        absolute_exp.as_str(),
                        if family.revoked { "1" } else { "0" },
                        family.last_attempt_id.as_deref().unwrap_or(""),
                        TENANT_INDEX_PREFIX,
                        TENANT_USER_INDEX_PREFIX,
                        KEY_PREFIX,
                    ],
                )
                .await
                .map_err(redis_unavailable)?;
            let code: i64 = redis::from_redis_value(result).map_err(redis_parse_unavailable)?;
            if code == 2 {
                return Err(AppError::Conflict("登录设备数量已达到安全上限".into()));
            }
        } else {
            let _mutation_guard = self.lock_local_mutations()?;
            let now = chrono::Utc::now().timestamp();
            let already_active = self.local.get(&family.sid).is_some_and(|existing| {
                !existing.revoked
                    && existing.absolute_exp > now
                    && existing.tenant_id == family.tenant_id
                    && existing.user_id == family.user_id
            });
            if !family.revoked && !already_active {
                let active_count = self
                    .local
                    .iter()
                    .filter(|entry| {
                        let existing = entry.value();
                        !existing.revoked
                            && existing.absolute_exp > now
                            && existing.tenant_id == family.tenant_id
                            && existing.user_id == family.user_id
                    })
                    .take(MAX_BULK_SESSION_CANDIDATES)
                    .count();
                if active_count >= MAX_BULK_SESSION_CANDIDATES {
                    return Err(AppError::Conflict("登录设备数量已达到安全上限".into()));
                }
            }
            self.local.insert(family.sid.clone(), family);
        }
        Ok(())
    }

    pub async fn rotate(
        &self,
        sid: &str,
        presented_jti: &str,
        new_jti: &str,
        now: i64,
        attempt_id: &str,
    ) -> AppResult<RefreshRotation> {
        if attempt_id.is_empty() {
            return Err(AppError::Authorization(
                "missing refresh rotation attempt id".into(),
            ));
        }
        if let Some(redis) = &self.redis {
            let key = family_key(sid);
            let now = now.to_string();
            let grace = CONCURRENT_GRACE_SECONDS.to_string();
            let result = redis
                .eval_script(
                    ROTATE_SCRIPT,
                    &[key.as_str()],
                    &[
                        presented_jti,
                        new_jti,
                        now.as_str(),
                        grace.as_str(),
                        attempt_id,
                        TENANT_INDEX_PREFIX,
                        TENANT_USER_INDEX_PREFIX,
                    ],
                )
                .await
                .map_err(redis_unavailable)?;
            let (code, current_jti, issued_at): (i64, String, i64) =
                redis::from_redis_value(result).map_err(redis_parse_unavailable)?;
            return Ok(match code {
                1 => RefreshRotation::Rotated {
                    current_jti,
                    issued_at,
                },
                2 => RefreshRotation::Concurrent,
                3 => RefreshRotation::Replayed,
                4 => RefreshRotation::Recovered {
                    current_jti,
                    issued_at,
                },
                _ => RefreshRotation::MissingOrRevoked,
            });
        }

        let _mutation_guard = self.lock_local_mutations()?;
        let Some(mut family) = self.local.get_mut(sid) else {
            return Ok(RefreshRotation::MissingOrRevoked);
        };
        if family.revoked || family.absolute_exp <= now {
            drop(family);
            self.local.remove(sid);
            return Ok(RefreshRotation::MissingOrRevoked);
        }
        if family.current_jti == presented_jti {
            family.previous_jti = Some(family.current_jti.clone());
            family.current_jti = new_jti.to_owned();
            family.rotated_at = now;
            family.last_attempt_id = Some(attempt_id.to_owned());
            return Ok(RefreshRotation::Rotated {
                current_jti: new_jti.to_owned(),
                issued_at: now,
            });
        }
        if family.previous_jti.as_deref() == Some(presented_jti) {
            if family.last_attempt_id.as_deref() == Some(attempt_id) {
                return Ok(RefreshRotation::Recovered {
                    current_jti: family.current_jti.clone(),
                    issued_at: family.rotated_at,
                });
            }
            if now - family.rotated_at <= CONCURRENT_GRACE_SECONDS {
                return Ok(RefreshRotation::Concurrent);
            }
        }
        family.revoked = true;
        Ok(RefreshRotation::Replayed)
    }

    pub async fn revoke(&self, sid: &str) -> AppResult<bool> {
        if sid.is_empty() {
            return Ok(false);
        }
        if let Some(redis) = &self.redis {
            let key = family_key(sid);
            let now = chrono::Utc::now().timestamp().to_string();
            let result = redis
                .eval_script(
                    REVOKE_SCRIPT,
                    &[key.as_str()],
                    &[now.as_str(), TENANT_INDEX_PREFIX, TENANT_USER_INDEX_PREFIX],
                )
                .await
                .map_err(redis_unavailable)?;
            return redis::from_redis_value(result).map_err(redis_parse_unavailable);
        }
        let now = chrono::Utc::now().timestamp();
        let _mutation_guard = self.lock_local_mutations()?;
        if let Some(mut family) = self.local.get_mut(sid) {
            if family.absolute_exp <= now {
                drop(family);
                self.local.remove(sid);
                return Ok(false);
            }
            family.revoked = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 原子验证刷新令牌族属于 `tenant_id` 后，以幂等方式撤销该令牌族。
    ///
    /// `false` 有意同时涵盖令牌族不存在和租户不匹配，调用方不能借此枚举其他租户的会话。
    /// Redis 失败仍可区分为 503。
    pub async fn revoke_for_tenant(&self, tenant_id: &str, sid: &str) -> AppResult<bool> {
        if tenant_id.is_empty() || sid.is_empty() {
            return Ok(false);
        }
        if let Some(redis) = &self.redis {
            let key = family_key(sid);
            let now = chrono::Utc::now().timestamp().to_string();
            let result = redis
                .eval_script(
                    REVOKE_FOR_TENANT_SCRIPT,
                    &[key.as_str()],
                    &[
                        tenant_id,
                        now.as_str(),
                        TENANT_INDEX_PREFIX,
                        TENANT_USER_INDEX_PREFIX,
                    ],
                )
                .await
                .map_err(redis_unavailable)?;
            let code: i64 = redis::from_redis_value(result).map_err(redis_parse_unavailable)?;
            return Ok(code == 1);
        }

        let now = chrono::Utc::now().timestamp();
        let _mutation_guard = self.lock_local_mutations()?;
        let Some(mut family) = self.local.get_mut(sid) else {
            return Ok(false);
        };
        if family.absolute_exp <= now {
            drop(family);
            self.local.remove(sid);
            return Ok(false);
        }
        if family.tenant_id != tenant_id {
            return Ok(false);
        }
        family.revoked = true;
        Ok(true)
    }

    /// 按租户和用户校验会话归属并原子撤销。
    ///
    /// 不存在、已过期或属于其他身份的会话统一返回 `NotFoundOrForeign`，避免借此枚举会话。
    pub async fn revoke_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        sid: &str,
    ) -> AppResult<RefreshSessionRevocation> {
        if tenant_id.is_empty() || sid.is_empty() {
            return Ok(RefreshSessionRevocation::NotFoundOrForeign);
        }
        let now = chrono::Utc::now().timestamp();
        if let Some(redis) = &self.redis {
            let key = family_key(sid);
            let user_id_arg = user_id.to_string();
            let now_arg = now.to_string();
            let result = redis
                .eval_script(
                    REVOKE_FOR_USER_SCRIPT,
                    &[key.as_str()],
                    &[
                        tenant_id,
                        user_id_arg.as_str(),
                        now_arg.as_str(),
                        TENANT_INDEX_PREFIX,
                        TENANT_USER_INDEX_PREFIX,
                    ],
                )
                .await
                .map_err(redis_unavailable)?;
            let code: i64 = redis::from_redis_value(result).map_err(redis_parse_unavailable)?;
            return Ok(match code {
                1 => RefreshSessionRevocation::Revoked,
                2 => RefreshSessionRevocation::AlreadyRevoked,
                _ => RefreshSessionRevocation::NotFoundOrForeign,
            });
        }

        let _mutation_guard = self.lock_local_mutations()?;
        let Some(mut family) = self.local.get_mut(sid) else {
            return Ok(RefreshSessionRevocation::NotFoundOrForeign);
        };
        if family.absolute_exp <= now {
            drop(family);
            self.local.remove(sid);
            return Ok(RefreshSessionRevocation::NotFoundOrForeign);
        }
        if family.tenant_id != tenant_id || family.user_id != user_id {
            return Ok(RefreshSessionRevocation::NotFoundOrForeign);
        }
        if family.revoked {
            return Ok(RefreshSessionRevocation::AlreadyRevoked);
        }
        family.revoked = true;
        Ok(RefreshSessionRevocation::Revoked)
    }

    /// 撤销指定用户除当前会话外的候选会话。
    ///
    /// Redis 模式在一个 Lua 脚本内完成所有校验、撤销和核心索引清理，避免连接故障导致部分成功；
    /// 内存模式使用所有 Store 实例共享的变更锁，确保整批操作不与其他本地会话变更交错。
    pub async fn revoke_other_sessions_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        current_sid: &str,
        candidate_sids: &[String],
    ) -> AppResult<u64> {
        if tenant_id.is_empty() || current_sid.is_empty() {
            return Err(AppError::Validation("会话身份不能为空".into()));
        }

        let mut candidates: Vec<&str> = candidate_sids
            .iter()
            .map(String::as_str)
            .filter(|sid| !sid.is_empty() && *sid != current_sid)
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        if candidates.len() > MAX_BULK_SESSION_CANDIDATES {
            return Err(AppError::Validation(format!(
                "一次最多撤销 {MAX_BULK_SESSION_CANDIDATES} 个登录设备"
            )));
        }
        let now = chrono::Utc::now().timestamp();
        if let Some(redis) = &self.redis {
            let user_id_arg = user_id.to_string();
            let now_arg = now.to_string();
            let mut args = Vec::with_capacity(candidates.len() + 7);
            args.extend_from_slice(&[
                tenant_id,
                user_id_arg.as_str(),
                current_sid,
                now_arg.as_str(),
                KEY_PREFIX,
                TENANT_INDEX_PREFIX,
                TENANT_USER_INDEX_PREFIX,
            ]);
            args.extend(candidates);
            let result = redis
                .eval_script(REVOKE_OTHER_SESSIONS_FOR_USER_SCRIPT, &[] as &[&str], &args)
                .await
                .map_err(redis_unavailable)?;
            let revoked_count: i64 =
                redis::from_redis_value(result).map_err(redis_parse_unavailable)?;
            if revoked_count == -1 {
                return Err(AppError::Validation(format!(
                    "一次最多撤销 {MAX_BULK_SESSION_CANDIDATES} 个登录设备"
                )));
            }
            return u64::try_from(revoked_count)
                .map_err(|_| redis_response_unavailable("invalid bulk revocation count"));
        }

        let _mutation_guard = self.lock_local_mutations()?;
        let mut authoritative_sids: Vec<String> = self
            .local
            .iter()
            .filter_map(|entry| {
                let family = entry.value();
                (!family.revoked
                    && family.absolute_exp > now
                    && family.tenant_id == tenant_id
                    && family.user_id == user_id
                    && family.sid != current_sid)
                    .then(|| family.sid.clone())
            })
            .collect();
        authoritative_sids.sort_unstable();
        authoritative_sids.dedup();
        if authoritative_sids.len() > MAX_BULK_SESSION_CANDIDATES {
            return Err(AppError::Validation(format!(
                "一次最多撤销 {MAX_BULK_SESSION_CANDIDATES} 个登录设备"
            )));
        }
        let mut revoked_count = 0_u64;
        let mut expired = Vec::new();
        for sid in authoritative_sids {
            let Some(mut family) = self.local.get_mut(&sid) else {
                continue;
            };
            if family.tenant_id != tenant_id || family.user_id != user_id {
                continue;
            }
            if family.absolute_exp <= now {
                expired.push(sid);
            } else if !family.revoked {
                family.revoked = true;
                revoked_count += 1;
            }
        }
        for sid in expired {
            self.local.remove(&sid);
        }
        Ok(revoked_count)
    }

    /// 返回会话当前的活跃身份；已撤销和已过期会话不会暴露身份信息。
    pub async fn identity(&self, sid: &str) -> AppResult<Option<RefreshSessionIdentity>> {
        if sid.is_empty() {
            return Ok(None);
        }
        let now = chrono::Utc::now().timestamp();
        if let Some(redis) = &self.redis {
            let key = family_key(sid);
            let now_arg = now.to_string();
            let result = redis
                .eval_script(
                    ACTIVE_IDENTITY_SCRIPT,
                    &[key.as_str()],
                    &[
                        now_arg.as_str(),
                        TENANT_INDEX_PREFIX,
                        TENANT_USER_INDEX_PREFIX,
                    ],
                )
                .await
                .map_err(redis_unavailable)?;
            let values: Vec<String> =
                redis::from_redis_value(result).map_err(redis_parse_unavailable)?;
            if values.is_empty() {
                return Ok(None);
            }
            if values.len() != 3 {
                return Err(redis_response_unavailable(
                    "invalid refresh identity response",
                ));
            }
            let user_id = values[1]
                .parse::<i64>()
                .map_err(|_| redis_response_unavailable("invalid refresh identity user id"))?;
            let absolute_exp = values[2]
                .parse::<i64>()
                .map_err(|_| redis_response_unavailable("invalid refresh identity expiry"))?;
            return Ok(Some(RefreshSessionIdentity {
                tenant_id: values[0].clone(),
                user_id,
                absolute_exp,
            }));
        }

        let _mutation_guard = self.lock_local_mutations()?;
        let Some(family) = self.local.get(sid) else {
            return Ok(None);
        };
        if family.absolute_exp <= now {
            drop(family);
            self.local.remove(sid);
            return Ok(None);
        }
        if family.revoked {
            return Ok(None);
        }
        Ok(Some(RefreshSessionIdentity {
            tenant_id: family.tenant_id.clone(),
            user_id: family.user_id,
            absolute_exp: family.absolute_exp,
        }))
    }

    /// 在一次 Redis 脚本或一次本地读锁中校验会话及其完整身份。
    pub async fn is_active_for_identity(
        &self,
        sid: &str,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<bool> {
        if sid.is_empty() || tenant_id.is_empty() {
            return Ok(false);
        }
        let now = chrono::Utc::now().timestamp();
        if let Some(redis) = &self.redis {
            let key = family_key(sid);
            let user_id_arg = user_id.to_string();
            let now_arg = now.to_string();
            let result = redis
                .eval_script(
                    IS_ACTIVE_FOR_IDENTITY_SCRIPT,
                    &[key.as_str()],
                    &[
                        tenant_id,
                        user_id_arg.as_str(),
                        now_arg.as_str(),
                        TENANT_INDEX_PREFIX,
                        TENANT_USER_INDEX_PREFIX,
                    ],
                )
                .await
                .map_err(redis_unavailable)?;
            return redis::from_redis_value(result).map_err(redis_parse_unavailable);
        }

        let _mutation_guard = self.lock_local_mutations()?;
        let Some(family) = self.local.get(sid) else {
            return Ok(false);
        };
        let active = !family.revoked
            && family.absolute_exp > now
            && family.tenant_id == tenant_id
            && family.user_id == user_id;
        let expired = family.absolute_exp <= now;
        drop(family);
        if expired {
            self.local.remove(sid);
        }
        Ok(active)
    }

    /// 从新索引读取指定租户用户的活跃会话 SID。
    ///
    /// 升级前会话尚未写入该索引，上层在兼容期仍需合并旧在线会话索引并逐个调用 `identity` 校验。
    pub async fn session_sids_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<Vec<String>> {
        if tenant_id.is_empty() {
            return Ok(Vec::new());
        }
        let now = chrono::Utc::now().timestamp();
        if let Some(redis) = &self.redis {
            let user_index = tenant_user_index_key(tenant_id, user_id);
            let tenant_index = tenant_index_key(tenant_id);
            let user_id_arg = user_id.to_string();
            let now_arg = now.to_string();
            let result = redis
                .eval_script(
                    USER_SESSION_SIDS_SCRIPT,
                    &[user_index.as_str(), tenant_index.as_str()],
                    &[
                        tenant_id,
                        user_id_arg.as_str(),
                        KEY_PREFIX,
                        now_arg.as_str(),
                    ],
                )
                .await
                .map_err(redis_unavailable)?;
            return parse_sorted_sids(result);
        }

        let _mutation_guard = self.lock_local_mutations()?;
        Ok(self.local_session_sids(now, |family| {
            family.tenant_id == tenant_id && family.user_id == user_id
        }))
    }

    /// 从新索引读取指定租户的活跃会话 SID。
    pub async fn session_sids_for_tenant(&self, tenant_id: &str) -> AppResult<Vec<String>> {
        if tenant_id.is_empty() {
            return Ok(Vec::new());
        }
        let now = chrono::Utc::now().timestamp();
        if let Some(redis) = &self.redis {
            let tenant_index = tenant_index_key(tenant_id);
            let now_arg = now.to_string();
            let result = redis
                .eval_script(
                    TENANT_SESSION_SIDS_SCRIPT,
                    &[tenant_index.as_str()],
                    &[
                        tenant_id,
                        KEY_PREFIX,
                        now_arg.as_str(),
                        TENANT_USER_INDEX_PREFIX,
                    ],
                )
                .await
                .map_err(redis_unavailable)?;
            return parse_sorted_sids(result);
        }

        let _mutation_guard = self.lock_local_mutations()?;
        Ok(self.local_session_sids(now, |family| family.tenant_id == tenant_id))
    }

    fn local_session_sids(
        &self,
        now: i64,
        predicate: impl Fn(&RefreshFamily) -> bool,
    ) -> Vec<String> {
        let mut expired = Vec::new();
        let mut active = Vec::new();
        for entry in self.local.iter() {
            let family = entry.value();
            if family.absolute_exp <= now {
                expired.push(entry.key().clone());
            } else if !family.revoked && predicate(family) {
                active.push(entry.key().clone());
            }
        }
        for sid in expired {
            self.local.remove(&sid);
        }
        active.sort_unstable();
        active
    }

    pub async fn is_active(&self, sid: &str) -> AppResult<bool> {
        Ok(self.identity(sid).await?.is_some())
    }
}

fn family_key(sid: &str) -> String {
    format!("{KEY_PREFIX}{sid}")
}

fn tenant_index_key(tenant_id: &str) -> String {
    format!("{TENANT_INDEX_PREFIX}{tenant_id}")
}

fn tenant_user_index_key(tenant_id: &str, user_id: i64) -> String {
    format!("{TENANT_USER_INDEX_PREFIX}{tenant_id}:{user_id}")
}

fn parse_sorted_sids(value: redis::Value) -> AppResult<Vec<String>> {
    let mut sids: Vec<String> = redis::from_redis_value(value).map_err(redis_parse_unavailable)?;
    sids.sort_unstable();
    sids.dedup();
    Ok(sids)
}

fn remaining_ttl(absolute_exp: i64) -> AppResult<u64> {
    let remaining = absolute_exp - chrono::Utc::now().timestamp();
    if remaining <= 0 {
        return Err(AppError::Authentication("refresh session expired".into()));
    }
    Ok(remaining as u64)
}

fn redis_unavailable(error: redis::RedisError) -> AppError {
    tracing::error!(%error, "refresh session Redis operation failed");
    AppError::ServiceUnavailable("session service unavailable".into())
}

fn redis_parse_unavailable(error: redis::ParsingError) -> AppError {
    tracing::error!(%error, "refresh session Redis response parsing failed");
    AppError::ServiceUnavailable("session service unavailable".into())
}

fn redis_response_unavailable(message: &str) -> AppError {
    tracing::error!(message, "refresh session Redis response is invalid");
    AppError::ServiceUnavailable("session service unavailable".into())
}
