use std::collections::{BTreeSet, HashMap};

use redis::{AsyncCommands, aio::ConnectionLike};
use ryframe_kernel::AppResult;

use crate::RedisNamespace;

use super::{RefreshFamily, RefreshSessionIdentity, codec, keyspace};

pub(super) fn scoped_family(scope: &RedisNamespace, sid: &str) -> String {
    scope.key(&keyspace::family(sid))
}

pub(super) fn scoped_tenant_index(scope: &RedisNamespace, tenant_id: &str) -> String {
    scope.key(&keyspace::tenant_index(tenant_id))
}

pub(super) fn scoped_tenant_user_index(
    scope: &RedisNamespace,
    tenant_id: &str,
    user_id: i64,
) -> String {
    scope.key(&keyspace::tenant_user_index(tenant_id, user_id))
}

async fn redis_type<C>(connection: &mut C, key: &str) -> Result<String, redis::RedisError>
where
    C: ConnectionLike + Send + Sync,
{
    redis::cmd("TYPE").arg(key).query_async(connection).await
}

pub(super) async fn ensure_types<C>(
    connection: &mut C,
    keys: &[(&String, &str)],
) -> Result<(), redis::RedisError>
where
    C: ConnectionLike + Send + Sync,
{
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

pub(super) async fn watch_additional<C>(
    connection: &mut C,
    keys: &[String],
) -> Result<(), redis::RedisError>
where
    C: ConnectionLike + Send + Sync,
{
    if !keys.is_empty() {
        redis::cmd("WATCH").arg(keys).exec_async(connection).await?;
    }
    Ok(())
}

pub(super) async fn load_family<C>(
    connection: &mut C,
    key: &str,
) -> Result<Option<RefreshFamily>, redis::RedisError>
where
    C: ConnectionLike + Send + Sync,
{
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

pub(super) fn queue_family_write(
    transaction: &mut redis::Pipeline,
    key: &str,
    family: &RefreshFamily,
) {
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

pub(super) async fn queue_expiry_extension<C>(
    connection: &mut C,
    transaction: &mut redis::Pipeline,
    key: &str,
    absolute_exp: i64,
) -> Result<(), redis::RedisError>
where
    C: ConnectionLike + Send + Sync,
{
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

pub(super) async fn queue_index_removal<C>(
    connection: &mut C,
    transaction: &mut redis::Pipeline,
    key: &str,
    sid: &str,
) -> Result<(), redis::RedisError>
where
    C: ConnectionLike + Send + Sync,
{
    queue_index_removals(connection, transaction, key, &[sid.to_owned()]).await
}

pub(super) async fn queue_index_removals<C>(
    connection: &mut C,
    transaction: &mut redis::Pipeline,
    key: &str,
    sids: &[String],
) -> Result<(), redis::RedisError>
where
    C: ConnectionLike + Send + Sync,
{
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

pub(super) fn parse_identity(values: Vec<String>) -> AppResult<Option<RefreshSessionIdentity>> {
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
