use std::collections::{BTreeSet, HashMap};

use redis::AsyncCommands;
use ryframe_kernel::{AppError, AppResult};

use crate::{RedisClient, RedisNamespace};

use super::{
    CONCURRENT_GRACE_SECONDS, MAX_BULK_SESSION_CANDIDATES, RefreshFamily, RefreshRotation,
    RefreshSessionIdentity, RefreshSessionRevocation, codec, keyspace,
};

#[derive(Clone)]
pub(super) struct RedisRefreshSessionStore {
    client: RedisClient,
}

impl RedisRefreshSessionStore {
    pub(super) fn new(client: RedisClient) -> Self {
        Self { client }
    }

    pub(super) async fn register(&self, family: &RefreshFamily) -> AppResult<()> {
        let scope = self.client.keyspace();
        let family_key = scoped_family(&scope, &family.sid);
        let tenant_key = scoped_tenant_index(&scope, &family.tenant_id);
        let user_key = scoped_tenant_user_index(&scope, &family.tenant_id, family.user_id);
        let watched = [family_key.clone(), tenant_key.clone(), user_key.clone()];
        let family = family.clone();
        let code = self
            .client
            .transaction(&watched, move |mut connection, mut transaction| {
                let family = family.clone();
                let family_key = family_key.clone();
                let tenant_key = tenant_key.clone();
                let user_key = user_key.clone();
                let scope = scope.clone();
                async move {
                    ensure_types(
                        &mut connection,
                        &[
                            (&family_key, "hash"),
                            (&tenant_key, "set"),
                            (&user_key, "set"),
                        ],
                    )
                    .await?;

                    let indexed_sids: Vec<String> = connection.smembers(&user_key).await?;
                    if indexed_sids.len() > MAX_BULK_SESSION_CANDIDATES {
                        return Ok(Some(2_i64));
                    }
                    let indexed_family_keys = indexed_sids
                        .iter()
                        .map(|sid| scoped_family(&scope, sid))
                        .collect::<Vec<_>>();
                    watch_additional(&mut connection, &indexed_family_keys).await?;

                    let now = family.rotated_at;
                    let mut stale_sids = BTreeSet::new();
                    let mut active_count = 0_usize;
                    let mut new_sid_indexed = false;
                    for (sid, indexed_key) in indexed_sids.iter().zip(&indexed_family_keys) {
                        ensure_types(&mut connection, &[(indexed_key, "hash")]).await?;
                        let Some(indexed) = load_family(&mut connection, indexed_key).await? else {
                            stale_sids.insert(sid.clone());
                            continue;
                        };
                        if indexed.tenant_id == family.tenant_id
                            && indexed.user_id == family.user_id
                            && indexed.absolute_exp > now
                            && !indexed.revoked
                        {
                            active_count += 1;
                            new_sid_indexed |= indexed.sid == family.sid;
                        } else {
                            stale_sids.insert(sid.clone());
                        }
                    }
                    if !family.revoked
                        && !new_sid_indexed
                        && active_count >= MAX_BULK_SESSION_CANDIDATES
                    {
                        return Ok(Some(2_i64));
                    }

                    if let Some(old) = load_family(&mut connection, &family_key).await? {
                        let old_tenant_key = scoped_tenant_index(&scope, &old.tenant_id);
                        let old_user_key =
                            scoped_tenant_user_index(&scope, &old.tenant_id, old.user_id);
                        watch_additional(
                            &mut connection,
                            &[old_tenant_key.clone(), old_user_key.clone()],
                        )
                        .await?;
                        ensure_types(
                            &mut connection,
                            &[(&old_tenant_key, "set"), (&old_user_key, "set")],
                        )
                        .await?;
                        transaction.srem(&old_tenant_key, &family.sid).ignore();
                        transaction.srem(&old_user_key, &family.sid).ignore();
                    }
                    for stale_sid in stale_sids {
                        transaction.srem(&tenant_key, &stale_sid).ignore();
                        transaction.srem(&user_key, &stale_sid).ignore();
                    }
                    queue_family_write(&mut transaction, &family_key, &family);
                    if !family.revoked {
                        transaction.sadd(&tenant_key, &family.sid).ignore();
                        transaction.sadd(&user_key, &family.sid).ignore();
                        queue_expiry_extension(
                            &mut connection,
                            &mut transaction,
                            &tenant_key,
                            family.absolute_exp,
                        )
                        .await?;
                        queue_expiry_extension(
                            &mut connection,
                            &mut transaction,
                            &user_key,
                            family.absolute_exp,
                        )
                        .await?;
                    }
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| 1_i64))
                }
            })
            .await
            .map_err(codec::redis_unavailable)?;
        if code == 2 {
            return Err(AppError::Conflict("登录设备数量已达到安全上限".into()));
        }
        Ok(())
    }

    pub(super) async fn rotate(
        &self,
        sid: &str,
        presented_jti: &str,
        new_jti: &str,
        now: i64,
        attempt_id: &str,
    ) -> AppResult<RefreshRotation> {
        let scope = self.client.keyspace();
        let family_key = scoped_family(&scope, sid);
        let watched = [family_key.clone()];
        let presented_jti = presented_jti.to_owned();
        let new_jti = new_jti.to_owned();
        let attempt_id = attempt_id.to_owned();
        let result: (i64, String, i64) = self
            .client
            .transaction(&watched, move |mut connection, mut transaction| {
                let family_key = family_key.clone();
                let presented_jti = presented_jti.clone();
                let new_jti = new_jti.clone();
                let attempt_id = attempt_id.clone();
                let scope = scope.clone();
                async move {
                    ensure_types(&mut connection, &[(&family_key, "hash")]).await?;
                    let Some(family) = load_family(&mut connection, &family_key).await? else {
                        return Ok(Some((0, String::new(), 0)));
                    };
                    if family.revoked {
                        return Ok(Some((0, String::new(), 0)));
                    }
                    let tenant_key = scoped_tenant_index(&scope, &family.tenant_id);
                    let user_key =
                        scoped_tenant_user_index(&scope, &family.tenant_id, family.user_id);
                    watch_additional(&mut connection, &[tenant_key.clone(), user_key.clone()])
                        .await?;
                    ensure_types(&mut connection, &[(&tenant_key, "set"), (&user_key, "set")])
                        .await?;

                    let outcome = if family.absolute_exp <= now {
                        transaction.del(&family_key).ignore();
                        queue_index_removal(
                            &mut connection,
                            &mut transaction,
                            &tenant_key,
                            &family.sid,
                        )
                        .await?;
                        queue_index_removal(
                            &mut connection,
                            &mut transaction,
                            &user_key,
                            &family.sid,
                        )
                        .await?;
                        (0, String::new(), 0)
                    } else if family.current_jti == presented_jti {
                        transaction
                            .cmd("HSET")
                            .arg(&family_key)
                            .arg("previous_jti")
                            .arg(&family.current_jti)
                            .arg("current_jti")
                            .arg(&new_jti)
                            .arg("rotated_at")
                            .arg(now)
                            .arg("last_attempt_id")
                            .arg(&attempt_id)
                            .ignore();
                        transaction
                            .cmd("EXPIREAT")
                            .arg(&family_key)
                            .arg(family.absolute_exp)
                            .ignore();
                        (1, new_jti.clone(), now)
                    } else if family.previous_jti.as_deref() == Some(presented_jti.as_str())
                        && family.last_attempt_id.as_deref() == Some(attempt_id.as_str())
                    {
                        return Ok(Some((4, family.current_jti, family.rotated_at)));
                    } else if family.previous_jti.as_deref() == Some(presented_jti.as_str())
                        && now - family.rotated_at <= CONCURRENT_GRACE_SECONDS
                    {
                        return Ok(Some((2, String::new(), 0)));
                    } else {
                        transaction.hset(&family_key, "revoked", "1").ignore();
                        transaction
                            .cmd("EXPIREAT")
                            .arg(&family_key)
                            .arg(family.absolute_exp)
                            .ignore();
                        queue_index_removal(
                            &mut connection,
                            &mut transaction,
                            &tenant_key,
                            &family.sid,
                        )
                        .await?;
                        queue_index_removal(
                            &mut connection,
                            &mut transaction,
                            &user_key,
                            &family.sid,
                        )
                        .await?;
                        (3, String::new(), 0)
                    };
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| outcome))
                }
            })
            .await
            .map_err(codec::redis_unavailable)?;
        Ok(match result.0 {
            1 => RefreshRotation::Rotated {
                current_jti: result.1,
                issued_at: result.2,
            },
            2 => RefreshRotation::Concurrent,
            3 => RefreshRotation::Replayed,
            4 => RefreshRotation::Recovered {
                current_jti: result.1,
                issued_at: result.2,
            },
            _ => RefreshRotation::MissingOrRevoked,
        })
    }

    pub(super) async fn revoke(&self, sid: &str, now: i64) -> AppResult<bool> {
        Ok(self.revoke_owned(None, None, sid, now).await? == 1)
    }

    pub(super) async fn revoke_for_tenant(
        &self,
        tenant_id: &str,
        sid: &str,
        now: i64,
    ) -> AppResult<bool> {
        Ok(self.revoke_owned(Some(tenant_id), None, sid, now).await? == 1)
    }

    pub(super) async fn revoke_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        sid: &str,
        now: i64,
    ) -> AppResult<RefreshSessionRevocation> {
        Ok(
            match self
                .revoke_owned(Some(tenant_id), Some(user_id), sid, now)
                .await?
            {
                1 => RefreshSessionRevocation::Revoked,
                2 => RefreshSessionRevocation::AlreadyRevoked,
                _ => RefreshSessionRevocation::NotFoundOrForeign,
            },
        )
    }

    async fn revoke_owned(
        &self,
        tenant_id: Option<&str>,
        user_id: Option<i64>,
        sid: &str,
        now: i64,
    ) -> AppResult<i64> {
        let scope = self.client.keyspace();
        let family_key = scoped_family(&scope, sid);
        let watched = [family_key.clone()];
        let tenant_id = tenant_id.map(str::to_owned);
        let sid = sid.to_owned();
        self.client
            .transaction(&watched, move |mut connection, mut transaction| {
                let family_key = family_key.clone();
                let tenant_id = tenant_id.clone();
                let sid = sid.clone();
                let scope = scope.clone();
                async move {
                    ensure_types(&mut connection, &[(&family_key, "hash")]).await?;
                    let Some(family) = load_family(&mut connection, &family_key).await? else {
                        return Ok(Some(0_i64));
                    };
                    if tenant_id
                        .as_deref()
                        .is_some_and(|value| value != family.tenant_id)
                        || user_id.is_some_and(|value| value != family.user_id)
                    {
                        return Ok(Some(0_i64));
                    }
                    let tenant_key = scoped_tenant_index(&scope, &family.tenant_id);
                    let user_key =
                        scoped_tenant_user_index(&scope, &family.tenant_id, family.user_id);
                    watch_additional(&mut connection, &[tenant_key.clone(), user_key.clone()])
                        .await?;
                    ensure_types(&mut connection, &[(&tenant_key, "set"), (&user_key, "set")])
                        .await?;
                    let code = if family.absolute_exp <= now {
                        transaction.del(&family_key).ignore();
                        0
                    } else if family.revoked && user_id.is_some() {
                        2
                    } else {
                        transaction.hset(&family_key, "revoked", "1").ignore();
                        transaction
                            .cmd("EXPIREAT")
                            .arg(&family_key)
                            .arg(family.absolute_exp)
                            .ignore();
                        1
                    };
                    queue_index_removal(&mut connection, &mut transaction, &tenant_key, &sid)
                        .await?;
                    queue_index_removal(&mut connection, &mut transaction, &user_key, &sid).await?;
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| code))
                }
            })
            .await
            .map_err(codec::redis_unavailable)
    }

    pub(super) async fn revoke_other_sessions_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        current_sid: &str,
        candidates: Vec<&str>,
        now: i64,
    ) -> AppResult<u64> {
        let scope = self.client.keyspace();
        let tenant_key = scoped_tenant_index(&scope, tenant_id);
        let user_key = scoped_tenant_user_index(&scope, tenant_id, user_id);
        let watched = [tenant_key.clone(), user_key.clone()];
        let tenant_id = tenant_id.to_owned();
        let current_sid = current_sid.to_owned();
        let candidates = candidates
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let count: i64 = self
            .client
            .transaction(&watched, move |mut connection, mut transaction| {
                let tenant_key = tenant_key.clone();
                let user_key = user_key.clone();
                let tenant_id = tenant_id.clone();
                let current_sid = current_sid.clone();
                let candidates = candidates.clone();
                let scope = scope.clone();
                async move {
                    ensure_types(&mut connection, &[(&tenant_key, "set"), (&user_key, "set")])
                        .await?;
                    let indexed: Vec<String> = connection.smembers(&user_key).await?;
                    let mut all = candidates
                        .into_iter()
                        .chain(indexed)
                        .collect::<BTreeSet<_>>();
                    all.remove(&current_sid);
                    if all.len() > MAX_BULK_SESSION_CANDIDATES {
                        return Ok(Some(-1_i64));
                    }
                    let family_keys = all
                        .iter()
                        .map(|sid| scoped_family(&scope, sid))
                        .collect::<Vec<_>>();
                    watch_additional(&mut connection, &family_keys).await?;
                    let mut revoked = 0_i64;
                    let mut owned = Vec::new();
                    for (sid, family_key) in all.iter().zip(&family_keys) {
                        ensure_types(&mut connection, &[(family_key, "hash")]).await?;
                        let Some(family) = load_family(&mut connection, family_key).await? else {
                            continue;
                        };
                        if family.tenant_id != tenant_id || family.user_id != user_id {
                            continue;
                        }
                        owned.push(sid.clone());
                        if family.absolute_exp <= now {
                            transaction.del(family_key).ignore();
                        } else if !family.revoked {
                            transaction.hset(family_key, "revoked", "1").ignore();
                            transaction
                                .cmd("EXPIREAT")
                                .arg(family_key)
                                .arg(family.absolute_exp)
                                .ignore();
                            revoked += 1;
                        }
                    }
                    queue_index_removals(&mut connection, &mut transaction, &tenant_key, &owned)
                        .await?;
                    queue_index_removals(&mut connection, &mut transaction, &user_key, &owned)
                        .await?;
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| revoked))
                }
            })
            .await
            .map_err(codec::redis_unavailable)?;
        if count < 0 {
            return Err(AppError::Validation(format!(
                "一次最多撤销 {MAX_BULK_SESSION_CANDIDATES} 个登录设备"
            )));
        }
        u64::try_from(count)
            .map_err(|_| codec::redis_response_unavailable("invalid bulk revocation count"))
    }

    pub(super) async fn identity(
        &self,
        sid: &str,
        now: i64,
    ) -> AppResult<Option<RefreshSessionIdentity>> {
        let scope = self.client.keyspace();
        let family_key = scoped_family(&scope, sid);
        let watched = [family_key.clone()];
        let values: Vec<String> = self
            .client
            .transaction(&watched, move |mut connection, mut transaction| {
                let family_key = family_key.clone();
                let scope = scope.clone();
                async move {
                    ensure_types(&mut connection, &[(&family_key, "hash")]).await?;
                    let Some(family) = load_family(&mut connection, &family_key).await? else {
                        return Ok(Some(Vec::new()));
                    };
                    if !family.revoked && family.absolute_exp > now {
                        return Ok(Some(vec![
                            family.tenant_id,
                            family.user_id.to_string(),
                            family.absolute_exp.to_string(),
                        ]));
                    }
                    let tenant_key = scoped_tenant_index(&scope, &family.tenant_id);
                    let user_key =
                        scoped_tenant_user_index(&scope, &family.tenant_id, family.user_id);
                    watch_additional(&mut connection, &[tenant_key.clone(), user_key.clone()])
                        .await?;
                    ensure_types(&mut connection, &[(&tenant_key, "set"), (&user_key, "set")])
                        .await?;
                    if family.absolute_exp <= now {
                        transaction.del(&family_key).ignore();
                    }
                    queue_index_removal(
                        &mut connection,
                        &mut transaction,
                        &tenant_key,
                        &family.sid,
                    )
                    .await?;
                    queue_index_removal(&mut connection, &mut transaction, &user_key, &family.sid)
                        .await?;
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| Vec::new()))
                }
            })
            .await
            .map_err(codec::redis_unavailable)?;
        parse_identity(values)
    }

    pub(super) async fn is_active_for_identity(
        &self,
        sid: &str,
        tenant_id: &str,
        user_id: i64,
        now: i64,
    ) -> AppResult<bool> {
        Ok(self
            .identity(sid, now)
            .await?
            .is_some_and(|identity| identity.tenant_id == tenant_id && identity.user_id == user_id))
    }

    pub(super) async fn session_sids_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        now: i64,
    ) -> AppResult<Vec<String>> {
        self.session_sids(tenant_id, Some(user_id), now).await
    }

    pub(super) async fn session_sids_for_tenant(
        &self,
        tenant_id: &str,
        now: i64,
    ) -> AppResult<Vec<String>> {
        self.session_sids(tenant_id, None, now).await
    }

    async fn session_sids(
        &self,
        tenant_id: &str,
        user_id: Option<i64>,
        now: i64,
    ) -> AppResult<Vec<String>> {
        let scope = self.client.keyspace();
        let tenant_key = scoped_tenant_index(&scope, tenant_id);
        let primary_key = user_id
            .map(|user_id| scoped_tenant_user_index(&scope, tenant_id, user_id))
            .unwrap_or_else(|| tenant_key.clone());
        let watched = [primary_key.clone(), tenant_key.clone()];
        let tenant_id = tenant_id.to_owned();
        self.client
            .transaction(&watched, move |mut connection, mut transaction| {
                let primary_key = primary_key.clone();
                let tenant_key = tenant_key.clone();
                let tenant_id = tenant_id.clone();
                let scope = scope.clone();
                async move {
                    ensure_types(
                        &mut connection,
                        &[(&primary_key, "set"), (&tenant_key, "set")],
                    )
                    .await?;
                    let sids: Vec<String> = connection.smembers(&primary_key).await?;
                    let family_keys = sids
                        .iter()
                        .map(|sid| scoped_family(&scope, sid))
                        .collect::<Vec<_>>();
                    watch_additional(&mut connection, &family_keys).await?;
                    let mut active = Vec::new();
                    let mut stale = Vec::new();
                    for (sid, family_key) in sids.iter().zip(&family_keys) {
                        ensure_types(&mut connection, &[(family_key, "hash")]).await?;
                        let family = load_family(&mut connection, family_key).await?;
                        let is_active = family.as_ref().is_some_and(|family| {
                            family.tenant_id == tenant_id
                                && user_id.is_none_or(|value| family.user_id == value)
                                && !family.revoked
                                && family.absolute_exp > now
                        });
                        if is_active {
                            active.push(sid.clone());
                        } else {
                            stale.push(sid.clone());
                            if family
                                .as_ref()
                                .is_some_and(|family| family.absolute_exp <= now)
                            {
                                transaction.del(family_key).ignore();
                            }
                        }
                    }
                    if stale.is_empty() {
                        active.sort_unstable();
                        active.dedup();
                        return Ok(Some(active));
                    }
                    queue_index_removals(&mut connection, &mut transaction, &primary_key, &stale)
                        .await?;
                    if primary_key != tenant_key {
                        queue_index_removals(
                            &mut connection,
                            &mut transaction,
                            &tenant_key,
                            &stale,
                        )
                        .await?;
                    }
                    active.sort_unstable();
                    active.dedup();
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| active))
                }
            })
            .await
            .map_err(codec::redis_unavailable)
    }
}

fn scoped_family(scope: &RedisNamespace, sid: &str) -> String {
    scope.key(&keyspace::family(sid))
}

fn scoped_tenant_index(scope: &RedisNamespace, tenant_id: &str) -> String {
    scope.key(&keyspace::tenant_index(tenant_id))
}

fn scoped_tenant_user_index(scope: &RedisNamespace, tenant_id: &str, user_id: i64) -> String {
    scope.key(&keyspace::tenant_user_index(tenant_id, user_id))
}

async fn redis_type(
    connection: &mut redis::aio::MultiplexedConnection,
    key: &str,
) -> Result<String, redis::RedisError> {
    redis::cmd("TYPE").arg(key).query_async(connection).await
}

async fn ensure_types(
    connection: &mut redis::aio::MultiplexedConnection,
    keys: &[(&String, &str)],
) -> Result<(), redis::RedisError> {
    for (key, expected) in keys {
        let actual = redis_type(connection, key).await?;
        if actual != "none" && actual != *expected {
            return Err(redis::RedisError::from((
                redis::ErrorKind::UnexpectedReturnType,
                "invalid refresh session key type",
                format!("expected {expected}, received {actual}"),
            )));
        }
    }
    Ok(())
}

async fn watch_additional(
    connection: &mut redis::aio::MultiplexedConnection,
    keys: &[String],
) -> Result<(), redis::RedisError> {
    if !keys.is_empty() {
        redis::cmd("WATCH").arg(keys).exec_async(connection).await?;
    }
    Ok(())
}

async fn load_family(
    connection: &mut redis::aio::MultiplexedConnection,
    key: &str,
) -> Result<Option<RefreshFamily>, redis::RedisError> {
    let fields: HashMap<String, String> = connection.hgetall(key).await?;
    if fields.is_empty() {
        return Ok(None);
    }
    Ok(Some(RefreshFamily {
        sid: required(&fields, "sid")?.to_owned(),
        tenant_id: required(&fields, "tenant_id")?.to_owned(),
        user_id: parse_field(&fields, "user_id")?,
        current_jti: required(&fields, "current_jti")?.to_owned(),
        previous_jti: optional(&fields, "previous_jti"),
        last_attempt_id: optional(&fields, "last_attempt_id"),
        rotated_at: parse_field(&fields, "rotated_at")?,
        absolute_exp: parse_field(&fields, "absolute_exp")?,
        revoked: match required(&fields, "revoked")? {
            "0" => false,
            "1" => true,
            _ => return Err(invalid_family("revoked")),
        },
    }))
}

fn required<'a>(
    fields: &'a HashMap<String, String>,
    name: &'static str,
) -> Result<&'a str, redis::RedisError> {
    fields
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| invalid_family(name))
}

fn optional(fields: &HashMap<String, String>, name: &str) -> Option<String> {
    fields.get(name).filter(|value| !value.is_empty()).cloned()
}

fn parse_field<T: std::str::FromStr>(
    fields: &HashMap<String, String>,
    name: &'static str,
) -> Result<T, redis::RedisError> {
    required(fields, name)?
        .parse()
        .map_err(|_| invalid_family(name))
}

fn invalid_family(field: &'static str) -> redis::RedisError {
    redis::RedisError::from((
        redis::ErrorKind::UnexpectedReturnType,
        "invalid refresh session fields",
        field.to_owned(),
    ))
}

fn queue_family_write(transaction: &mut redis::Pipeline, key: &str, family: &RefreshFamily) {
    transaction
        .cmd("HSET")
        .arg(key)
        .arg("sid")
        .arg(&family.sid)
        .arg("tenant_id")
        .arg(&family.tenant_id)
        .arg("user_id")
        .arg(family.user_id)
        .arg("current_jti")
        .arg(&family.current_jti)
        .arg("previous_jti")
        .arg(family.previous_jti.as_deref().unwrap_or(""))
        .arg("rotated_at")
        .arg(family.rotated_at)
        .arg("absolute_exp")
        .arg(family.absolute_exp)
        .arg("revoked")
        .arg(if family.revoked { "1" } else { "0" })
        .arg("last_attempt_id")
        .arg(family.last_attempt_id.as_deref().unwrap_or(""))
        .ignore();
    transaction
        .cmd("EXPIREAT")
        .arg(key)
        .arg(family.absolute_exp)
        .ignore();
}

async fn queue_expiry_extension(
    connection: &mut redis::aio::MultiplexedConnection,
    transaction: &mut redis::Pipeline,
    key: &str,
    absolute_exp: i64,
) -> Result<(), redis::RedisError> {
    let current: i64 = redis::cmd("EXPIRETIME")
        .arg(key)
        .query_async(connection)
        .await?;
    if current < absolute_exp {
        transaction
            .cmd("EXPIREAT")
            .arg(key)
            .arg(absolute_exp)
            .ignore();
    }
    Ok(())
}

async fn queue_index_removal(
    connection: &mut redis::aio::MultiplexedConnection,
    transaction: &mut redis::Pipeline,
    key: &str,
    sid: &str,
) -> Result<(), redis::RedisError> {
    queue_index_removals(connection, transaction, key, &[sid.to_owned()]).await
}

async fn queue_index_removals(
    connection: &mut redis::aio::MultiplexedConnection,
    transaction: &mut redis::Pipeline,
    key: &str,
    sids: &[String],
) -> Result<(), redis::RedisError> {
    if sids.is_empty() {
        return Ok(());
    }
    let members: Vec<String> = connection.smembers(key).await?;
    let removed = sids.iter().collect::<BTreeSet<_>>();
    if members.iter().all(|member| removed.contains(member)) {
        transaction.del(key).ignore();
    } else {
        transaction.srem(key, sids).ignore();
    }
    Ok(())
}

fn parse_identity(values: Vec<String>) -> AppResult<Option<RefreshSessionIdentity>> {
    if values.is_empty() {
        return Ok(None);
    }
    if values.len() != 3 {
        return Err(codec::redis_response_unavailable(
            "invalid refresh identity response",
        ));
    }
    let user_id = values[1]
        .parse::<i64>()
        .map_err(|_| codec::redis_response_unavailable("invalid refresh identity user id"))?;
    let absolute_exp = values[2]
        .parse::<i64>()
        .map_err(|_| codec::redis_response_unavailable("invalid refresh identity expiry"))?;
    Ok(Some(RefreshSessionIdentity {
        tenant_id: values[0].clone(),
        user_id,
        absolute_exp,
    }))
}
