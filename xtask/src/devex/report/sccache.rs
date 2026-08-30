use std::{collections::BTreeMap, fs, path::Path};

use serde::Deserialize;

use crate::Result;

use super::SccacheDelta;

#[derive(Debug, Clone, Deserialize)]
struct Counters {
    compile_requests: u64,
    cache_hits: CounterGroup,
    cache_misses: CounterGroup,
    #[serde(rename = "requests_not_cacheable")]
    not_cacheable: u64,
    not_cached: BTreeMap<String, u64>,
    cache_errors: CounterGroup,
    cache_timeouts: u64,
    cache_read_errors: u64,
    cache_write_errors: u64,
    dist_errors: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct CounterGroup {
    counts: BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
struct StatsDocument {
    stats: Counters,
}

pub(super) fn read_delta(run_dir: &Path, required: bool) -> Result<Option<SccacheDelta>> {
    let before_path = run_dir.join("sccache-before.json");
    let after_path = run_dir.join("sccache-after.json");
    if !before_path.is_file() && !after_path.is_file() {
        return if required {
            Err("sccache suite 缺少统计快照".into())
        } else {
            Ok(None)
        };
    }
    if !before_path.is_file() || !after_path.is_file() {
        return Err("sccache 统计快照不完整".into());
    }
    delta(
        parse(&fs::read(before_path)?)?,
        parse(&fs::read(after_path)?)?,
    )
    .map(Some)
}

fn parse(document: &[u8]) -> Result<Counters> {
    Ok(serde_json::from_slice::<StatsDocument>(document)?.stats)
}

fn delta(before: Counters, after: Counters) -> Result<SccacheDelta> {
    let cache_hits = group_delta(&before.cache_hits, &after.cache_hits, "cache_hits")?;
    let cache_misses = group_delta(&before.cache_misses, &after.cache_misses, "cache_misses")?;
    let cacheable = cache_hits
        .checked_add(cache_misses)
        .ok_or("sccache 可缓存请求计数溢出")?;
    let cache_errors = checked_sum([
        group_delta(&before.cache_errors, &after.cache_errors, "cache_errors")?,
        counter_delta(
            before.cache_timeouts,
            after.cache_timeouts,
            "cache_timeouts",
        )?,
        counter_delta(
            before.cache_read_errors,
            after.cache_read_errors,
            "cache_read_errors",
        )?,
        counter_delta(
            before.cache_write_errors,
            after.cache_write_errors,
            "cache_write_errors",
        )?,
        counter_delta(before.dist_errors, after.dist_errors, "dist_errors")?,
    ])?;
    let not_cacheable = counter_delta(
        before.not_cacheable,
        after.not_cacheable,
        "requests_not_cacheable",
    )?;
    let not_cached = counter_map_deltas(&before.not_cached, &after.not_cached, "not_cached")?;
    let categorized_not_cacheable = checked_sum(not_cached.values().copied())?;
    if categorized_not_cacheable != not_cacheable {
        return Err(format!(
            "sccache 不可缓存请求分类不一致：总数 {not_cacheable}，分类 {categorized_not_cacheable}"
        )
        .into());
    }
    let not_cacheable_control_probes = control_probe_count(&not_cached)?;
    Ok(SccacheDelta {
        compile_requests: counter_delta(
            before.compile_requests,
            after.compile_requests,
            "compile_requests",
        )?,
        cache_hits,
        cache_misses,
        not_cacheable,
        not_cacheable_control_probes,
        not_cacheable_compilations: not_cacheable
            .checked_sub(not_cacheable_control_probes)
            .ok_or("sccache 控制探测计数超过不可缓存请求")?,
        cache_errors,
        hit_rate: (cacheable > 0).then(|| cache_hits as f64 / cacheable as f64),
    })
}

fn counter_delta(before: u64, after: u64, name: &str) -> Result<u64> {
    after.checked_sub(before).ok_or_else(|| {
        format!("sccache 计数器 `{name}` 回退：before={before}，after={after}").into()
    })
}

fn group_delta(before: &CounterGroup, after: &CounterGroup, name: &str) -> Result<u64> {
    checked_sum(group_deltas(before, after, name)?.into_values())
}

fn group_deltas(
    before: &CounterGroup,
    after: &CounterGroup,
    name: &str,
) -> Result<BTreeMap<String, u64>> {
    counter_map_deltas(&before.counts, &after.counts, name)
}

fn counter_map_deltas(
    before: &BTreeMap<String, u64>,
    after: &BTreeMap<String, u64>,
    name: &str,
) -> Result<BTreeMap<String, u64>> {
    let mut deltas = BTreeMap::new();
    for (category, before_value) in before {
        let after_value = after.get(category).copied().unwrap_or(0);
        let value = counter_delta(
            *before_value,
            after_value,
            &format!("{name}.counts.{category}"),
        )?;
        deltas.insert(category.clone(), value);
    }
    for (category, after_value) in after {
        if !before.contains_key(category) {
            deltas.insert(category.clone(), *after_value);
        }
    }
    Ok(deltas)
}

fn control_probe_count(not_cached: &BTreeMap<String, u64>) -> Result<u64> {
    checked_sum(
        ["-", "missing input"]
            .into_iter()
            .map(|reason| not_cached.get(reason).copied().unwrap_or(0)),
    )
}

fn checked_sum(values: impl IntoIterator<Item = u64>) -> Result<u64> {
    values.into_iter().try_fold(0_u64, |total, value| {
        total
            .checked_add(value)
            .ok_or_else(|| "sccache 错误计数增量溢出".into())
    })
}
