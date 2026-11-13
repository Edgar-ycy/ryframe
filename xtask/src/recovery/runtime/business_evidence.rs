//! 用已核验权威独立重建浏览器业务证明。

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::inputs::{
        document::{self, Document},
        domain,
    },
};

const REQUIRED_SCENARIOS: [&str; 9] = [
    "login",
    "session",
    "post",
    "notice",
    "tenant",
    "export",
    "message",
    "schedule",
    "restored-data",
];

pub(in crate::recovery) struct Evidence<'a> {
    pub(in crate::recovery) proof: &'a Document,
    pub(in crate::recovery) tests: &'a Document,
    pub(in crate::recovery) runtime: &'a Document,
    pub(in crate::recovery) target: &'a Document,
}

pub(in crate::recovery) struct Source<'a> {
    pub(in crate::recovery) root: &'a Path,
    pub(in crate::recovery) sha: &'a str,
}

pub(in crate::recovery) struct Authority<'a> {
    pub(in crate::recovery) record: &'a Value,
    pub(in crate::recovery) runtime: &'a Value,
    pub(in crate::recovery) coordinator: Source<'a>,
    pub(in crate::recovery) runner: Source<'a>,
    pub(in crate::recovery) verifier: Source<'a>,
    pub(in crate::recovery) source_generation: &'a Value,
    pub(in crate::recovery) dataset_lineage: &'a Value,
}

/// 调用方在读取窗口内保护输入文件，且已独立核验源码、运行和数据权威。
pub(in crate::recovery) fn verify<'a>(
    evidence: Evidence<'a>,
    authority: Authority<'_>,
) -> Result<&'a Value> {
    let id = document::text(&authority.record["plan"]["id"], "恢复演练 ID")?;
    verify_evidence_paths(&evidence, authority.runner.root, id)?;
    let restore = json!({
        "id": id,
        "plan_hash": authority.record["plan_hash"],
        "scope_id": authority.record["plan"]["scope_id"],
    });
    let sources = source_fields(&authority)?;
    verify_tests(&evidence, &authority, &restore, &sources)?;
    let expected = expected_proof(&evidence, &authority, &restore, &sources);
    if evidence.proof.value != expected {
        return Err("恢复业务证明与测试明细或权威来源不一致".into());
    }
    Ok(&evidence.proof.value)
}

pub(crate) fn restore_evidence_paths(runner: &Path, id: &str) -> Result<[PathBuf; 3]> {
    domain::identifier(id)?;
    let directory = runner.join(".local-tests/playwright-real");
    Ok([
        directory.join(format!("restore-{id}.json")),
        directory.join(format!("restore-{id}-tests.json")),
        directory.join(format!("restore-{id}-runtime.json")),
    ])
}

fn verify_evidence_paths(evidence: &Evidence<'_>, runner: &Path, id: &str) -> Result<()> {
    for (document, expected) in [evidence.proof, evidence.tests, evidence.runtime]
        .into_iter()
        .zip(restore_evidence_paths(runner, id)?)
    {
        if document.path != expected {
            return Err("恢复业务证明、运行副本与测试明细必须位于 runner 的隔离证据目录".into());
        }
    }
    Ok(())
}

fn source_binding(source: &Source<'_>, label: &str) -> Result<Value> {
    let sha = domain::hash_40(&Value::from(source.sha), label)?;
    let root = source
        .root
        .to_str()
        .filter(|_| source.root.is_absolute())
        .ok_or_else(|| format!("{label}必须使用明确绝对源码根"))?;
    Ok(json!({"root": root, "sha": sha}))
}

fn source_fields(authority: &Authority<'_>) -> Result<Value> {
    let runtime = authority.runtime;
    Ok(json!({
        "backup_source_sha": runtime["backup_source_sha"],
        "backend_product_sha": runtime["backend_product_sha"],
        "backend_execution_sha": runtime["backend_execution_sha"],
        "backend_adapter_contract": runtime["backend_adapter_contract"],
        "frontend_sha": runtime["frontend_sha"],
        "coordinator": source_binding(&authority.coordinator, "恢复证明协调后端")?,
        "runner": source_binding(&authority.runner, "恢复测试 runner")?,
        "verifier": source_binding(&authority.verifier, "恢复证明核验器")?,
    }))
}

fn verify_tests(
    evidence: &Evidence<'_>,
    authority: &Authority<'_>,
    restore: &Value,
    sources: &Value,
) -> Result<()> {
    let receipt = &evidence.tests.value;
    document::exact(
        receipt,
        &[
            "format_version",
            "kind",
            "restore",
            "runtime",
            "target_plan",
            "source_generation",
            "dataset_lineage",
            "sources",
            "frontend_url",
            "started_at",
            "completed_at",
            "runs",
        ],
        "恢复浏览器测试收据",
    )?;
    verify_runs(&receipt["runs"])?;
    let runtime = document::binding(evidence.runtime);
    let target = document::binding(evidence.target);
    for (field, expected) in [
        ("runtime", &runtime),
        ("target_plan", &target),
        ("source_generation", authority.source_generation),
        ("dataset_lineage", authority.dataset_lineage),
    ] {
        document::binding_shape(expected, field)?;
        if &receipt[field] != expected {
            return Err(format!("恢复浏览器测试收据的 {field} 与权威文件描述不一致").into());
        }
    }
    if receipt["format_version"].as_u64() != Some(1)
        || receipt["kind"] != "restore-browser-tests"
        || &receipt["restore"] != restore
        || &receipt["sources"] != sources
        || receipt["frontend_url"] != authority.runtime["frontend_endpoint"]
    {
        return Err("恢复浏览器测试收据与权威运行、目标或源码不一致".into());
    }
    domain::timestamp(&receipt["started_at"], "恢复浏览器测试开始时间")?;
    domain::timestamp(&receipt["completed_at"], "恢复浏览器测试完成时间")?;
    Ok(())
}

fn verify_runs(value: &Value) -> Result<()> {
    let runs = document::array(value, "恢复浏览器测试明细")?;
    if runs.is_empty() {
        return Err("恢复浏览器测试明细不能为空".into());
    }
    let mut observed = BTreeSet::new();
    for run in runs {
        document::exact(
            run,
            &["title", "status", "retry", "scenarios"],
            "恢复浏览器测试明细",
        )?;
        let title = document::array(&run["title"], "恢复浏览器测试标题")?;
        if title.is_empty()
            || title
                .iter()
                .any(|part| part.as_str().is_none_or(|part| part.trim().is_empty()))
            || run["status"] != "passed"
            || run["retry"].as_u64() != Some(0)
        {
            return Err("恢复浏览器测试包含失败、重试或无效标题".into());
        }
        for scenario in document::array(&run["scenarios"], "恢复浏览器测试场景")? {
            let name = document::text(scenario, "恢复浏览器测试场景")?;
            if !REQUIRED_SCENARIOS.contains(&name) || !observed.insert(name) {
                return Err("恢复浏览器测试场景额外或重复".into());
            }
        }
    }
    if observed.len() != REQUIRED_SCENARIOS.len() {
        return Err("恢复浏览器测试场景缺失".into());
    }
    Ok(())
}

fn expected_proof(
    evidence: &Evidence<'_>,
    authority: &Authority<'_>,
    restore: &Value,
    sources: &Value,
) -> Value {
    let tests = &evidence.tests.value;
    let scenarios = REQUIRED_SCENARIOS.map(|name| json!({"name": name, "succeeded": true}));
    json!({
        "restore_id": restore["id"],
        "plan_hash": restore["plan_hash"],
        "backup_source_sha": sources["backup_source_sha"],
        "backend_product_sha": sources["backend_product_sha"],
        "backend_execution_sha": sources["backend_execution_sha"],
        "backend_adapter_contract": sources["backend_adapter_contract"],
        "frontend_sha": sources["frontend_sha"],
        "runner_sha": authority.runner.sha,
        "verifier_sha": authority.verifier.sha,
        "scope_id": restore["scope_id"],
        "frontend_url": authority.runtime["frontend_endpoint"],
        "runtime_receipt_sha256": evidence.runtime.sha256(),
        "tests_receipt_sha256": evidence.tests.sha256(),
        "target_plan_sha256": evidence.target.sha256(),
        "source_generation_sha256": authority.source_generation["sha256"],
        "dataset_lineage_sha256": authority.dataset_lineage["sha256"],
        "started_at": tests["started_at"],
        "completed_at": tests["completed_at"],
        "scenarios": scenarios,
        "unexpected_console_messages": 0,
        "unexpected_network_failures": 0,
        "axe_serious_or_critical": 0,
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime/business_evidence.rs"]
mod tests;
