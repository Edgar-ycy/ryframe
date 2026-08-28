use super::RunSummary;

pub(super) fn render(summary: &RunSummary) -> String {
    let metrics = summary.duration_ms.as_ref().map_or_else(
        || "无成功测量样本".to_owned(),
        |duration| {
            format!(
                "P50 {:.1} ms，P95 {:.1} ms，均值 {:.1} ms，范围 {:.1}–{:.1} ms",
                duration.p50, duration.p95, duration.mean, duration.min, duration.max
            )
        },
    );
    let sccache = summary.sccache.as_ref().map_or_else(String::new, |stats| {
        let hit_rate = stats
            .hit_rate
            .map_or_else(|| "n/a".to_owned(), |rate| format!("{:.1}%", rate * 100.0));
        format!(
            "- sccache：请求 {}，命中 {}，未命中 {}，不可缓存 {}，错误 {}，命中率 {}\n",
            stats.compile_requests,
            stats.cache_hits,
            stats.cache_misses,
            stats.not_cacheable,
            stats.cache_errors,
            hit_rate,
        )
    });
    let sccache_version = summary
        .sccache_version
        .as_deref()
        .map_or_else(String::new, |version| {
            format!("- sccache executable：`{version}`\n")
        });
    let pairing = summary
        .pairing
        .as_ref()
        .map_or_else(String::new, |pairing| {
            format!(
                "- paired：`{}` / `{}`\n",
                pairing.comparison_id,
                pairing.arm.as_str()
            )
        });
    format!(
        "# DevEx 测量摘要\n\n\
         - run：`{}`\n\
         - suite：`{}`\n\
         - variant：`{}`\n\
         - cache：`{}`\n\
         - execution surface：`{}`\n\
         - input：`{}`\n\
         {}\
         {}\
         - 样本：请求 {}，记录 {}（通过 {}，失败 {}）\n\
         - 耗时：{}\n\
         {}",
        summary.run_id,
        summary.suite.as_str(),
        summary.variant,
        summary.cache_state.as_str(),
        summary.compile_surface_fingerprint,
        summary.input_fingerprint,
        pairing,
        sccache_version,
        summary.requested_runs,
        summary.samples,
        summary.passed,
        summary.failed,
        metrics,
        sccache,
    )
}
