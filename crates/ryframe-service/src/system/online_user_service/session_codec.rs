use chrono::Utc;
use ryframe_common::{AppError, AppResult};

use super::{OnlineUserVo, UserSession, keyspace::session_key, session_to_vo};

pub(super) fn remaining_ttl(absolute_exp: i64) -> Option<u64> {
    let remaining = absolute_exp - Utc::now().timestamp();
    (remaining > 0).then_some(remaining as u64)
}

pub(super) fn encode(session: &UserSession) -> serde_json::Result<String> {
    serde_json::to_string(session)
}

pub(super) fn decode_batch(
    expected_tenant_id: &str,
    keys: &[String],
    values: Vec<Option<String>>,
) -> AppResult<Vec<OnlineUserVo>> {
    if keys.len() != values.len() {
        tracing::error!(
            key_count = keys.len(),
            value_count = values.len(),
            "Redis MGET 在线用户返回数量异常"
        );
        return Err(AppError::Internal("查询在线用户失败".into()));
    }

    let mut users = Vec::with_capacity(keys.len());
    for (key, value) in keys.iter().zip(values) {
        let Some(json) = value else {
            continue;
        };
        let session = serde_json::from_str::<UserSession>(&json).map_err(|error| {
            tracing::error!(%error, %key, "反序列化在线用户失败");
            AppError::Internal("在线用户数据损坏".into())
        })?;
        if session.tenant_id != expected_tenant_id
            || key != &session_key(expected_tenant_id, &session.sid)
        {
            tracing::warn!(
                %key,
                expected_tenant_id,
                session_tenant_id = session.tenant_id,
                "ignored an online-user index outside the requested tenant"
            );
            continue;
        }
        if remaining_ttl(session.absolute_exp).is_some() {
            users.push(session_to_vo(&session));
        }
    }
    Ok(users)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use ryframe_common::AppError;

    use super::{decode_batch, encode};
    use crate::system::online_user_service::{UserSession, keyspace::session_key};

    fn session(sid: &str, username: &str) -> UserSession {
        let now = Utc::now();
        UserSession {
            sid: sid.into(),
            tenant_id: "system".into(),
            user_id: 1,
            username: username.into(),
            dept_name: None,
            ipaddr: "127.0.0.1".into(),
            login_location: None,
            browser: None,
            os: None,
            login_time: now,
            last_access_time: now,
            absolute_exp: (now + Duration::hours(1)).timestamp(),
        }
    }

    #[test]
    fn batch_decode_preserves_key_order_and_skips_missing_entries() {
        let keys = vec![
            session_key("system", "b"),
            session_key("system", "expired"),
            session_key("system", "a"),
        ];
        let values = vec![
            Some(encode(&session("b", "bob")).unwrap()),
            None,
            Some(encode(&session("a", "alice")).unwrap()),
        ];

        let users = decode_batch("system", &keys, values).unwrap();

        assert_eq!(
            users
                .iter()
                .map(|user| user.username.as_str())
                .collect::<Vec<_>>(),
            ["bob", "alice"]
        );
    }

    #[test]
    fn batch_decode_rejects_corrupted_or_misaligned_data() {
        let corrupted = decode_batch("system", &["bad".into()], vec![Some("{".into())]);
        assert!(
            matches!(corrupted, Err(AppError::Internal(message)) if message == "在线用户数据损坏")
        );

        let misaligned = decode_batch("system", &["only-key".into()], Vec::new());
        assert!(
            matches!(misaligned, Err(AppError::Internal(message)) if message == "查询在线用户失败")
        );
    }

    #[test]
    fn batch_decode_filters_cross_tenant_and_mismatched_keys() {
        let foreign = UserSession {
            tenant_id: "tenant-b".into(),
            ..session("foreign", "mallory")
        };
        let wrong_key_session = session("actual", "eve");
        let keys = vec![
            session_key("system", "foreign"),
            session_key("system", "different"),
        ];
        let values = vec![
            Some(encode(&foreign).unwrap()),
            Some(encode(&wrong_key_session).unwrap()),
        ];

        assert!(decode_batch("system", &keys, values).unwrap().is_empty());
    }

    #[test]
    fn batch_decode_skips_expired_sessions() {
        let mut expired = session("expired", "alice");
        expired.absolute_exp = (Utc::now() - Duration::seconds(1)).timestamp();
        let key = session_key("system", &expired.sid);

        assert!(
            decode_batch("system", &[key], vec![Some(encode(&expired).unwrap())])
                .unwrap()
                .is_empty()
        );
    }
}
