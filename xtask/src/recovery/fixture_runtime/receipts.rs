use super::model::*;
use crate::{Result, recovery::clone::native::json as strict};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) const TESTS: [(&str, &[&str]); 4] = [
    (
        "真实业务数据从 shared-control 复制校验并切换到 shared",
        &["shared-migration"],
    ),
    (
        "真实业务数据从 dedicated-a 复制校验并切换到 dedicated-b",
        &["dedicated-migration", "retention"],
    ),
    (
        "真实排队业务迁移取消恢复源数据，并允许再次迁移",
        &["cancellation"],
    ),
    (
        "真实业务复制阻塞时 Worker 崩溃，重启后同一迁移恢复并完成校验",
        &["crash-recovery"],
    ),
];
pub(super) fn business(path: &Path, server: &str, run_id: &str) -> Result<Value> {
    let (v, guard) = document(path)?;
    strict::exact(
        &v,
        &[
            "format_version",
            "kind",
            "fixture",
            "server",
            "run_id",
            "status",
            "runs",
        ],
        "业务浏览器测试收据",
    )?;
    check(
        v["format_version"].as_u64() == Some(1)
            && v["kind"] == "business-browser-tests"
            && v["fixture"] == "business"
            && v["server"] == server
            && ["dev", "preview"].contains(&server)
            && v["run_id"] == run_id
            && Regex::new(r"^[a-z0-9][a-z0-9-]{0,63}$")?.is_match(run_id)
            && v["status"] == "passed",
        "浏览器测试收据与本次运行不一致",
    )?;
    let runs = v["runs"].as_array().ok_or("浏览器测试明细无效")?;
    check(runs.len() == 4, "浏览器测试必须包含精确四次成功运行")?;
    let mut titles = BTreeSet::new();
    for run in runs {
        strict::exact(
            run,
            &["title", "status", "retry", "scenarios"],
            "业务浏览器测试明细",
        )?;
        let title = run["title"]
            .as_array()
            .filter(|a| a.len() == 1)
            .and_then(|a| a[0].as_str())
            .ok_or("浏览器测试无效标题")?;
        let expected = TESTS
            .iter()
            .find(|(t, _)| *t == title)
            .ok_or("浏览器测试未知标题")?;
        check(
            run["status"] == "passed"
                && run["retry"].as_u64() == Some(0)
                && run["scenarios"] == json!(expected.1)
                && titles.insert(title),
            "浏览器测试包含失败、重试或场景缺失重复",
        )?;
    }
    unchanged(&guard)?;
    Ok(json!({"receipt":v,"descriptor":guard["descriptor"]}))
}
pub(super) fn budget(path: &Path, binding: &Value) -> Result<Value> {
    let (v, guard) = document(path)?;
    strict::exact(
        &v,
        &["version", "binding", "observedAt", "buckets"],
        "登录预算账本",
    )?;
    let capacity = binding["rate_limits"]["login"]["capacity"]
        .as_u64()
        .ok_or("登录容量无效")?;
    let window = binding["rate_limits"]["login"]["window_secs"]
        .as_u64()
        .ok_or("登录窗口无效")?
        .checked_mul(1000)
        .ok_or("登录窗口溢出")?;
    let observed = v["observedAt"].as_u64().ok_or("登录账本时间无效")?;
    let buckets = v["buckets"].as_object().ok_or("登录预算条目无效")?;
    check(
        v["version"].as_u64() == Some(1)
            && v["binding"]
                == json!({"scope":binding["scope_id"],"capacity":capacity,"windowMs":window})
            && observed as u128 <= SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
            && !buckets.is_empty()
            && buckets.len() <= 1024,
        "登录预算与scope、限流或首次浏览器写入不一致",
    )?;
    let key = Regex::new(r"^(principal|ip):[a-f0-9]{64}$")?;
    let generation = Regex::new(r"^[a-f0-9-]{36}$")?;
    for (k, b) in buckets {
        strict::exact(
            b,
            &["generation", "count", "reservedAt", "completedAt"],
            "登录预算条目",
        )?;
        let count = b["count"].as_u64().ok_or("登录次数无效")?;
        let reserved = b["reservedAt"].as_u64().ok_or("登录保留时间无效")?;
        let completed = &b["completedAt"];
        check(
            key.is_match(k)
                && generation.is_match(text(&b["generation"])?)
                && count > 0
                && count <= capacity
                && reserved <= observed
                && (completed.is_null()
                    || completed
                        .as_u64()
                        .is_some_and(|t| reserved <= t && t <= observed)),
            "登录预算账本包含损坏条目",
        )?;
    }
    unchanged(&guard)?;
    Ok(json!({"ledger":v,"descriptor":guard["descriptor"]}))
}
