//! 由正式来源核验的主体顺序推导允许自然过期的 Redis 键。
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Result, recovery::clone::native::json as document};

const NAMESPACE: &str = "ryframe:{fixture-seed-wkc61111h}:";
const ORIGIN_TENANT_SCOPE: &str = "fixture-seed-wkc61108e";
const ACTORS: usize = 11;
const RATE_TTL_MS: i64 = 60_000;
const SNAPSHOT_TTL_MS: i64 = 300_000;

pub(in crate::recovery::source::evidence) fn expected(
    namespace: &str,
    identities: &Value,
    subjects: &Value,
) -> Result<BTreeMap<String, (&'static str, i64)>> {
    if namespace != NAMESPACE {
        return Err("来源短期 Redis 键计划仅接受原暖态 namespace".into());
    }
    let identities = document::array(identities, "来源短期 Redis 键计划的血缘身份")?;
    if identities.len() != ACTORS {
        return Err("来源短期 Redis 键计划必须绑定十一项顺序身份".into());
    }
    let plan = super::super::model::cache_plan(&json!({
        "namespace":namespace,"subjects":subjects
    }))?;
    let plan = document::array(&plan, "来源短期 Redis 键计划的会话主体")?;
    let mut users = BTreeSet::new();
    let mut keys = BTreeMap::new();
    for (index, (identity, subject)) in identities.iter().zip(plan).enumerate() {
        let tenant = document::text(&identity["tenant_id"], "来源短期键租户")?;
        let user = document::text(&subject["user_id"], "来源短期键用户")?;
        if tenant != tenant_for(index) || subject["tenant_id"] != tenant || !users.insert(user) {
            return Err("来源短期 Redis 键身份有重复或与血缘执行顺序不符".into());
        }
        let username = document::text(&identity["username"], "来源短期键的血缘用户名")?.trim();
        if username.is_empty() || username.chars().any(char::is_control) {
            return Err("来源短期 Redis 键的血缘用户名无效".into());
        }
        let snapshot = document::text(&subject["keys"]["snapshot"], "来源授权快照键")?;
        keys.insert(snapshot.to_owned(), ("hash", SNAPSHOT_TTL_MS));
        for key in rate_keys(tenant, username, index) {
            keys.insert(
                format!("{namespace}v0.5:rate-limit:{key}"),
                ("string", RATE_TTL_MS),
            );
        }
    }
    if keys.len() != ACTORS * 7 {
        return Err("来源短期 Redis 键计划有重复或缺失".into());
    }
    Ok(keys)
}

fn tenant_for(index: usize) -> String {
    if index == 0 {
        "system".into()
    } else {
        format!("{ORIGIN_TENANT_SCOPE}-{index:02}")
    }
}

fn rate_keys(tenant: &str, username: &str, index: usize) -> [String; 6] {
    let ip = format!("198.18.20.{}", index + 1);
    let principal = principal_digest(tenant, &username.to_lowercase());
    [
        format!("global:ip:{ip}"),
        format!("api:GET /api/v1/auth/csrf:ip:{ip}"),
        format!("api:POST /api/v1/auth/login:ip:{ip}"),
        format!("auth:login:principal:{principal}"),
        format!("auth:login:ip:{ip}"),
        format!("tenant:{tenant}"),
    ]
}

fn principal_digest(tenant: &str, normalized_username: &str) -> String {
    // 字节长度前缀与冻结产品 stable_scope_digest 一致。
    let mut digest = Sha256::new();
    for part in [tenant, normalized_username] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_transient_plans_exact_frozen_rate_and_snapshot_keys() {
        let (identities, subjects) = actors();
        let keys = expected(NAMESPACE, &identities, &subjects).unwrap();
        assert_eq!(keys.len(), 77);
        assert_eq!(
            keys.values().filter(|v| **v == ("string", 60_000)).count(),
            66
        );
        assert_eq!(
            keys.values().filter(|v| **v == ("hash", 300_000)).count(),
            11
        );
        for relative in [
            "global:ip:198.18.20.1",
            "api:GET /api/v1/auth/csrf:ip:198.18.20.1",
            "api:POST /api/v1/auth/login:ip:198.18.20.1",
            "auth:login:principal:4d19b3fca1f99c37ba3bfc9e388bb1c1c06045527607b9eaa2b9d20be280ecf6",
            "auth:login:ip:198.18.20.1",
            "tenant:system",
            "auth:login:ip:198.18.20.11",
            "tenant:fixture-seed-wkc61108e-10",
        ] {
            assert_eq!(
                keys.get(&format!("{NAMESPACE}v0.5:rate-limit:{relative}")),
                Some(&("string", 60_000))
            );
        }
        assert_eq!(
            keys.get("ryframe:{fixture-seed-wkc61111h}:authorization:{system}:user:1:snapshots:v4"),
            Some(&("hash", 300_000))
        );
        assert_eq!(
            keys.get("ryframe:{fixture-seed-wkc61111h}:authorization:{fixture-seed-wkc61108e-10}:user:11:snapshots:v4"),
            Some(&("hash", 300_000))
        );
        assert!(keys.keys().all(|key| !key.contains("demo_name")));
    }

    #[test]
    fn warm_transient_rejects_scope_order_duplicate_and_noncanonical_inputs() {
        let (identities, subjects) = actors();
        assert!(expected("ryframe:{different}:", &identities, &subjects).is_err());

        let mut reordered = identities.clone();
        reordered.as_array_mut().unwrap().swap(1, 2);
        let mut reordered_subjects = subjects.clone();
        reordered_subjects.as_array_mut().unwrap().swap(1, 2);
        assert!(expected(NAMESPACE, &reordered, &reordered_subjects).is_err());
        assert!(expected(NAMESPACE, &identities, &reordered_subjects).is_err());
        for id in [json!("1"), json!("02"), json!(2), json!("0")] {
            let mut invalid = subjects.clone();
            invalid[1]["user_id"] = id;
            assert!(expected(NAMESPACE, &identities, &invalid).is_err());
        }
        let mut missing = subjects;
        missing.as_array_mut().unwrap().pop();
        assert!(expected(NAMESPACE, &identities, &missing).is_err());
    }

    fn actors() -> (Value, Value) {
        let tenants: Vec<_> = (0..ACTORS).map(tenant_for).collect();
        let identities: Vec<_> = tenants
            .iter()
            .map(|tenant| json!({"tenant_id":tenant,"username":"  DeMo_Name  "}))
            .collect();
        let subjects: Vec<_> = tenants
            .iter()
            .enumerate()
            .map(|(index, tenant)| {
                json!({"tenant_id":tenant,"user_id":(index+1).to_string(),"user_authorization_version":2})
            })
            .collect();
        (json!(identities), json!(subjects))
    }
}
