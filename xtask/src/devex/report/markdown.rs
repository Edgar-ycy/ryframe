use super::{Distribution, RunSummary, acceptance::ComparisonCheck, percentage_change};

pub(super) fn render(summary: &RunSummary) -> String {
    let metrics = summary.duration_ms.as_ref().map_or_else(
        || "无成功测量样本".to_owned(),
        |duration| {
            format!(
                "P50 {:.1} ms，P95 {:.1} ms，P99 {:.1} ms，均值 {:.1} ms，范围 {:.1}–{:.1} ms",
                duration.p50, duration.p95, duration.p99, duration.mean, duration.min, duration.max
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
        .map_or_else(String::new, pairing_details);
    let resource_gate = if summary.resource_gate_targeted_decisions > 0 {
        format!(
            "- resource-gate targeted decision：{} 个测量样本\n",
            summary.resource_gate_targeted_decisions
        )
    } else {
        String::new()
    };
    let document = format!(
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
    );
    format!(
        "{document}{}{}",
        super::super::memory::render(summary.memory.as_ref()),
        runtime_metrics(summary)
    )
}

fn runtime_metrics(summary: &RunSummary) -> String {
    let Some(runtime) = &summary.runtime else {
        return String::new();
    };
    let mut result = format!(
        "\n## 运行时测量\n\nHTTP 请求 {}；业务周期成功 {}、失败 {}，周期失败率 {:.2}%；会话失败 {}，采集失败 {}。服务 RSS 采样峰值 {:.0} bytes，峰值 MySQL 连接 {:.0}，CPU P95 {:.2} seconds（以上统计包含有运行时证据的失败样本，无证据的初始化失败只列入外层失败样本数，不推算周期）。服务指标由已登记端点采集，不属于命令进程树内存。\n\n周期指一个完整业务流程或一次首页访问，不等于单个 HTTP 请求；首页多个指标共享同一周期，失败只累计一次。登录失败造成未完成的预定周期计入失败周期，退出失败单列为会话失败，均阻止通过。每个成功样本先计算周期分位数，再跨样本汇总；下表不是合并全部周期后的分位数。\n\n| 场景 | 周期 P50 的样本 P50 ms | 周期 P95 的样本 P95 ms | 周期 P99 的样本 P95 ms | 周期吞吐样本 P50 /s |\n|---|---:|---:|---:|---:|\n",
        runtime.network_requests,
        runtime.completed_cycles,
        runtime.failed_cycles,
        runtime.cycle_failure_rate * 100.0,
        runtime.session_failures,
        runtime.collector_failures,
        runtime.peak_resident_memory_bytes,
        runtime.peak_database_connections,
        runtime.cpu_seconds.p95
    );
    for (name, row) in &runtime.scenarios {
        result.push_str(&format!(
            "| {name} | {:.2} | {:.2} | {:.2} | {:.2} |\n",
            row.p50_ms.p50, row.p95_ms.p95, row.p99_ms.p95, row.throughput.p50
        ));
        if let (Some(queue), Some(execution)) = (&row.queue_p95_ms, &row.execution_p95_ms) {
            result.push_str(&format!(
                "| {name} 排队/执行 P95 的样本 P95 | — | {:.2}/{:.2} | — | — |\n",
                queue.p95, execution.p95
            ));
        }
    }
    result
}

pub(super) fn render_comparison(
    baseline: &RunSummary,
    candidate: &RunSummary,
    baseline_duration: &Distribution,
    candidate_duration: &Distribution,
    sccache_version: &str,
    checks: &[ComparisonCheck],
) -> String {
    let pairing = baseline
        .pairing
        .as_ref()
        .expect("ensure_comparable 已校验 pairing");
    let baseline_contract = pairing_contract_details(pairing);
    let mut document = format!(
        "# DevEx 对比\n\n\
         - suite：`{}`\n\
         - variant：`{}`\n\
         - cache：`{}`\n\
         - paired comparison：`{}`\n\
         {}\
         - execution surface：`{}`\n\
         - sccache executable：`{}`\n\
         - input：基线 `{}` / 候选 `{}`\n\n\
         | 指标 | 基线 | 候选 | 变化 |\n\
         | --- | ---: | ---: | ---: |\n\
         | P50 | {:.1} ms | {:.1} ms | {} |\n\
         | P95 | {:.1} ms | {:.1} ms | {} |\n\
         | P99 | {:.1} ms | {:.1} ms | {} |\n",
        baseline.suite.as_str(),
        baseline.variant,
        baseline.cache_state.as_str(),
        pairing.comparison_id,
        baseline_contract,
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
        baseline_duration.p99,
        candidate_duration.p99,
        percentage_change(baseline_duration.p99, candidate_duration.p99),
    );
    document.push_str(&super::super::memory::render_comparison(
        baseline.memory.as_ref(),
        candidate.memory.as_ref(),
    ));
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

fn pairing_details(pairing: &crate::devex::model::PairingMetadata) -> String {
    format!(
        "- paired：`{}` / `{}`\n{}",
        pairing.comparison_id,
        pairing.arm.as_str(),
        pairing_contract_details(pairing)
    )
}

fn pairing_contract_details(pairing: &crate::devex::model::PairingMetadata) -> String {
    let Some(contract) = pairing.baseline_contract else {
        return String::new();
    };
    let Some(provenance) = pairing.baseline_provenance.as_ref() else {
        return format!(
            "- baseline contract：`{}`（来源证据缺失）\n",
            contract.as_str()
        );
    };
    let frontend = provenance
        .frontend_commit
        .as_deref()
        .map_or_else(|| "n/a".to_owned(), ToOwned::to_owned);
    let paths = if provenance.adapter_paths.is_empty() {
        "n/a".to_owned()
    } else {
        provenance.adapter_paths.join(", ")
    };
    format!(
        "- baseline contract：`{}`\n- legacy adapter：backend `{}` → `{}`，frontend `{}`，patch `{}`，paths `{}`\n",
        contract.as_str(),
        provenance.base_commit,
        provenance.adapter_commit,
        frontend,
        provenance.patch_sha256,
        paths,
    )
}
