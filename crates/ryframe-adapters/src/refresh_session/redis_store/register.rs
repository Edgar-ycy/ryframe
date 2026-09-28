use std::collections::BTreeSet;

use redis::AsyncCommands;
use ryframe_kernel::{AppError, AppResult};

use super::super::MAX_BULK_SESSION_CANDIDATES;
use super::helpers::{
    ensure_types, load_family, queue_expiry_extension, queue_family_write, scoped_family,
    scoped_tenant_index, scoped_tenant_user_index, watch_additional,
};
use super::{RedisRefreshSessionStore, RefreshFamily, codec};

impl RedisRefreshSessionStore {
    pub(in crate::refresh_session) async fn register(
        &self,
        family: &RefreshFamily,
    ) -> AppResult<()> {
        let scope = self.client.keyspace();
        let family_key = scoped_family(&scope, &family.sid);
        let tenant_key = scoped_tenant_index(&scope, &family.tenant_id);
        let user_key = scoped_tenant_user_index(&scope, &family.tenant_id, family.user_id);
        let watched = [family_key.clone(), tenant_key.clone(), user_key.clone()];
        let family = family.clone();
        let code = self
            .client
            .transaction(&watched, async move |mut connection, mut transaction| {
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
                .await
            })
            .await
            .map_err(codec::redis_unavailable)?;
        if code == 2 {
            return Err(AppError::Conflict("登录设备数量已达到安全上限".into()));
        }
        Ok(())
    }
}
