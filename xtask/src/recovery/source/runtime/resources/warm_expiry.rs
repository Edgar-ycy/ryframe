//! 暖来源只观察本轮短期键自然到期，原授权和刷新会话状态均由前后像核对。
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use serde_json::{Value, json};

use super::{Resources, Transport, document, warm_redis, warm_transient};
use crate::{
    Result,
    recovery::clone::native::target_verify::redis::{self, Client},
    utc_timestamp::now_rfc3339,
};

type Plan = BTreeMap<String, (&'static str, i64)>;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Expiry {
    earliest_observation: i64,
    deadline: i64,
}

impl<T: Transport> Resources<'_, T> {
    pub(in crate::recovery::source::evidence) fn warm_expiry(
        &self,
        generation: &Value,
        before: &Value,
        identities: &Value,
        node: &Value,
    ) -> Result<Value> {
        if !warm_redis::applies(self.generation) {
            return Err("来源短期 Redis 键只允许在已登记暖来源观察".into());
        }
        let inputs = self.inputs();
        let scope = inputs.target_scope()?;
        let namespace = format!("ryframe:{{{scope}}}:");
        let stable = stable_keys(before, &namespace, &node["session_subjects"])?;
        let mut plan = warm_transient::expected(&namespace, identities, &node["subjects"])?;
        plan.retain(|key, _| !stable.contains(key));
        let original = document::object(&before["existing"], "暖来源 Redis 基线")?;
        let preserved_authorization_keys = original
            .keys()
            .filter(|name| {
                name.strip_prefix(&namespace)
                    .is_some_and(|name| name.starts_with("authorization:"))
            })
            .count();
        (self.checkpoint)()?;
        let mut client = Client::authenticated(&inputs, generation)?;
        let started_at = now_rfc3339();
        let (_, authorization_plan) = self.cache_plan(&node["subjects"])?;
        let (authorization, _) = self.cache_image(
            &node["subjects"],
            &namespace,
            &authorization_plan,
            &mut |parts| client.command(parts),
        )?;
        let mut expiries = BTreeMap::new();
        let mut first = true;
        loop {
            let image =
                warm_redis::read_markers(&inputs, &mut |parts| client.transaction_command(parts))?;
            let short = present(&image, before, &namespace, &stable, &plan)?;
            let now = warm_redis::server_time_ms(&client.transaction_command(&["TIME"])?)?;
            observe(&mut client, &plan, &short, &mut expiries, first, now)?;
            let after = warm_redis::server_time_ms(&client.transaction_command(&["TIME"])?)?;
            absence(&short, &expiries, after)?;
            #[cfg(windows)]
            client.recheck_windows_peer()?;
            if !redis::info_matches(
                generation,
                &client.transaction_command(&["INFO", "server"])?,
            )? {
                return Err("暖来源短期键观察期间 Redis 代次变化".into());
            }
            (self.checkpoint)()?;
            if short.is_empty() {
                let mut receipt = expiry_receipt(
                    &plan,
                    expiries,
                    preserved_authorization_keys,
                    started_at,
                    after,
                )?;
                receipt["authorization_observation"] = authorization;
                return Ok(receipt);
            }
            first = false;
            let next = short
                .iter()
                .filter_map(|key| expiries.get(key))
                .map(|entry| entry.deadline)
                .min();
            let remaining = next
                .unwrap_or(after)
                .saturating_sub(after)
                .clamp(100, 60_000);
            std::thread::sleep(Duration::from_millis(u64::try_from(remaining)?));
        }
    }
}

fn stable_keys(before: &Value, namespace: &str, sessions: &Value) -> Result<BTreeSet<String>> {
    let mut result: BTreeSet<_> = document::object(&before["existing"], "暖来源 Redis 基线")?
        .keys()
        .cloned()
        .collect();
    let sessions = document::array(sessions, "本轮来源会话")?;
    if sessions.len() != 11 {
        return Err("本轮来源短期键观察缺少十一条生产者会话".into());
    }
    for row in sessions {
        let sid = document::text(&row["sid_sha256"], "本轮来源会话摘要")?;
        if !result.insert(warm_redis::family_key(namespace, sid)) {
            return Err("来源生产者会话与原会话或彼此重复".into());
        }
    }
    Ok(result)
}

fn present(
    image: &Value,
    before: &Value,
    namespace: &str,
    stable: &BTreeSet<String>,
    plan: &Plan,
) -> Result<BTreeSet<String>> {
    if image["owner"] != before["owner"] || image["sentinel"] != before["sentinel"] {
        return Err("暖来源短期键观察期间原归属标记变化".into());
    }
    let mut actual = BTreeSet::new();
    let mut short = BTreeSet::new();
    for key in document::array(&image["keys"], "暖来源 Redis 实际键")? {
        let key = document::text(key, "暖来源 Redis 键")?;
        let name = warm_redis::observed_key(namespace, key)?;
        if stable.contains(&name) {
            actual.insert(name);
        } else if plan.contains_key(key) {
            short.insert(key.to_owned());
        } else {
            return Err("暖来源核验留下未登记的 Redis 键或会话索引".into());
        }
    }
    if actual != *stable {
        return Err("暖来源短期键观察缺少原条目或本轮注销会话".into());
    }
    Ok(short)
}

fn observe(
    client: &mut Client,
    plan: &Plan,
    short: &BTreeSet<String>,
    expiries: &mut BTreeMap<String, Expiry>,
    first: bool,
    now: i64,
) -> Result<()> {
    for key in short {
        let (kind, ttl) = plan[key];
        if client.transaction_command(&["MULTI"])? != "OK"
            || client.transaction_command(&["TYPE", key])? != "QUEUED"
            || client.transaction_command(&["PEXPIRETIME", key])? != "QUEUED"
        {
            return Err("暖来源短期键只读事务协议无效".into());
        }
        let result = client.transaction_command(&["EXEC"])?;
        let Some([actual_kind, deadline]) = result.as_array().map(Vec::as_slice) else {
            return Err("暖来源短期键只读事务结果无效".into());
        };
        if actual_kind == "none" && deadline.as_i64() == Some(-2) {
            continue;
        }
        let deadline = deadline.as_i64().ok_or("暖来源短期键绝对期限无效")?;
        if *actual_kind != kind || deadline <= now || deadline > now.saturating_add(ttl) {
            return Err("暖来源短期键类型或期限超出已登记范围".into());
        }
        let original = expiries.get(key);
        if !first && original.is_none_or(|entry| entry.deadline != deadline) {
            return Err("生产者退出后暖来源短期键出现或被续期".into());
        }
        expiries.entry(key.clone()).or_insert(Expiry {
            earliest_observation: now,
            deadline,
        });
    }
    Ok(())
}

fn absence(short: &BTreeSet<String>, entries: &BTreeMap<String, Expiry>, now: i64) -> Result<()> {
    if entries
        .iter()
        .any(|(key, expiry)| !short.contains(key) && now < expiry.deadline)
    {
        return Err("暖来源短期键在登记的自然到期前消失".into());
    }
    Ok(())
}

fn expiry_receipt(
    plan: &Plan,
    expiries: BTreeMap<String, Expiry>,
    preserved: usize,
    started_at: String,
    absent_at_ms: i64,
) -> Result<Value> {
    absence(&BTreeSet::new(), &expiries, absent_at_ms)?;
    let mut snapshots = Vec::new();
    let mut rates = Vec::new();
    for (key, entry) in expiries {
        let item = json!({"key":key,"observed_at_ms":entry.earliest_observation,
            "expires_at_ms":entry.deadline});
        match plan[&key].0 {
            "hash" => snapshots.push(item),
            _ => rates.push(item),
        }
    }
    Ok(
        json!({"format_version":1,"kind":"restore-source-warm-redis-expiry","status":"observed_absent",
        "started_at":started_at,"cleaned_at":now_rfc3339(),"absent_at_ms":absent_at_ms,
        "preserved_authorization_keys":preserved,"expired_snapshot_keys":snapshots,
        "expired_rate_limit_keys":rates,"remote_writes":0}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_expiry_rejects_early_disappearance_and_an_unregistered_key() {
        let namespace = "ryframe:{test-scope}:";
        let owner = format!("{namespace}.ryframe-owner");
        let allowed = format!("{namespace}v0.5:rate-limit:tenant:system");
        let plan = BTreeMap::from([(allowed.clone(), ("string", 60_000))]);
        let stable = BTreeSet::from([owner.clone()]);
        let before = json!({"owner":"owner","sentinel":"sentinel"});
        let mut observed = json!({"keys":[owner,allowed],"owner":"owner","sentinel":"sentinel"});
        let short = present(&observed, &before, namespace, &stable, &plan).unwrap();
        assert_eq!(short, BTreeSet::from([allowed.clone()]));
        let expiries = BTreeMap::from([(
            allowed,
            Expiry {
                earliest_observation: 100,
                deadline: 200,
            },
        )]);
        assert!(absence(&BTreeSet::new(), &expiries, 199).is_err());
        assert!(absence(&BTreeSet::new(), &expiries, 200).is_ok());
        observed["keys"][1] = json!(format!("{namespace}unknown"));
        assert!(present(&observed, &before, namespace, &stable, &plan).is_err());
    }
}
