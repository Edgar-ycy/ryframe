use super::{Distribution, RunSummary, acceptance::ComparisonCheck, percentage_change};

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
            "- sccache：请求 {}，命中 {}，未命中 {}，不可缓存原始 {}（控制探测 {}，实际编译 {}），错误 {}，命中率 {}\n",
            stats.compile_requests,
            stats.cache_hits,
            stats.cache_misses,
            stats.not_cacheable,
            stats.not_cacheable_control_probes,
            stats.not_cacheable_compilations,
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
    let resource_gate = if summary.resource_gate_targeted_decisions > 0 {
        format!(
            "- resource-gate targeted decision：{} 个测量样本\n",
            summary.resource_gate_targeted_decisions
        )
    } else {
        String::new()
    };
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
        resource_gate,
        summary.requested_runs,
        summary.samples,
        summary.passed,
        summary.failed,
        metrics,
        sccache,
    )
}

pub(super) fn render_comparison(
    baseline: &RunSummary,
    candidate: &RunSummary,
    baseline_duration: &Distribution,
    candidate_duration: &Distribution,
    sccache_version: &str,
    checks: &[ComparisonCheck],
) -> String {
    let mut document = format!(
        "# DevEx 对比\n\n\
         - suite：`{}`\n\
         - variant：`{}`\n\
         - cache：`{}`\n\
         - paired comparison：`{}`\n\
         - execution surface：`{}`\n\
         - sccache executable：`{}`\n\
         - input：基线 `{}` / 候选 `{}`\n\n\
         | 指标 | 基线 | 候选 | 变化 |\n\
         | --- | ---: | ---: | ---: |\n\
         | P50 | {:.1} ms | {:.1} ms | {} |\n\
         | P95 | {:.1} ms | {:.1} ms | {} |\n",
        baseline.suite.as_str(),
        baseline.variant,
        baseline.cache_state.as_str(),
        baseline
            .pairing
            .as_ref()
            .expect("ensure_comparable 已校验 pairing")
            .comparison_id,
        baseline.compile_surface_fingerprint,
        sccache_version,
        baseline.input_fingerprint,
        candidate.input_fingerprint,
        baseline_duration.p50,
        candidate_duration.p50,
        percentage_change(baseline_duration.p50, candidate_duration.p50),
        baseline_duration.p95,
        candidate_duration.p95,
        percentage_change(baseline_duration.p95, candidate_duration.p95),
    );
    if checks.is_empty() {
        return document;
    }
    document
        .push_str("\n## 验收判定\n\n| 门禁 | 要求 | 实测 | 结果 |\n| --- | --- | --- | --- |\n");
    for check in checks {
        use std::fmt::Write;
        writeln!(
            document,
            "| {} | {} | {} | {} |",
            check.name,
            check.requirement,
            check.observed,
            if check.passed { "通过" } else { "失败" }
        )
        .expect("写入 String 不会失败");
    }
    document.push_str(if checks.iter().all(|check| check.passed) {
        "\n- 总判定：通过\n"
    } else {
        "\n- 总判定：失败\n"
    });
    document
}
