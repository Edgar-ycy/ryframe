use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::Result;

use super::support::sha256_shape;

pub(crate) const EMPTY_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
pub(crate) const BUSINESS_BACKEND_OUTPUTS: &[&str] = &[
    "crates/order-business/migrations/m_resource_initial_order.rs",
    "crates/order-business/src/generated/entities/order.rs",
    "crates/order-business/src/generated/handlers/order.rs",
    "crates/order-business/src/generated/openapi/order.rs",
    "crates/order-business/src/resources/mod.rs",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Requirement {
    pub(crate) repository: String,
    pub(crate) workflow: String,
    pub(crate) sha: String,
    pub(crate) jobs: Vec<String>,
    pub(crate) tag: Option<String>,
}

pub(crate) fn latest_run<'a>(runs: &'a [Value], requirement: &Requirement) -> Option<&'a Value> {
    runs.iter()
        .filter(|run| {
            run.get("head_sha").and_then(Value::as_str) == Some(requirement.sha.as_str())
                && run.get("event").and_then(Value::as_str) == Some("push")
                && requirement
                    .tag
                    .as_deref()
                    .is_none_or(|tag| run.get("head_branch").and_then(Value::as_str) == Some(tag))
        })
        .max_by_key(|run| {
            (
                run.get("run_number").and_then(Value::as_u64).unwrap_or(0),
                run.get("id").and_then(Value::as_u64).unwrap_or(0),
                run.get("run_attempt").and_then(Value::as_u64).unwrap_or(0),
            )
        })
}

pub(crate) fn validate_run(
    run: &Value,
    jobs: &[Value],
    requirement: &Requirement,
    jobs_attempt: u64,
) -> Result<Option<Value>> {
    if run.get("head_sha").and_then(Value::as_str) != Some(requirement.sha.as_str()) {
        return Err("运行的源码 SHA 与发布目标不一致".into());
    }
    if run.get("event").and_then(Value::as_str) != Some("push")
        || requirement
            .tag
            .as_deref()
            .is_some_and(|tag| run.get("head_branch").and_then(Value::as_str) != Some(tag))
    {
        return Err("运行不是目标协调 tag 的 push 验收".into());
    }
    let run_id = positive_integer(run.get("id"), "运行 ID")?;
    let attempt = positive_integer(run.get("run_attempt"), "运行 attempt")?;
    let status = run.get("status").and_then(Value::as_str).unwrap_or("");
    if is_pending_status(status) {
        return Ok(None);
    }
    let conclusion = run.get("conclusion").and_then(Value::as_str);
    if status != "completed" || conclusion != Some("success") {
        return Err(format!(
            "运行 {run_id} / {attempt} 未成功：{}",
            conclusion.unwrap_or("null")
        )
        .into());
    }
    if jobs_attempt != attempt {
        return Err("job 列表不属于当前运行尝试".into());
    }
    for job in jobs {
        validate_job_identity(job, &requirement.sha, run_id, jobs_attempt)?;
    }
    if jobs.iter().any(job_is_pending) {
        return Ok(None);
    }
    if jobs
        .iter()
        .any(|job| job.get("status").and_then(Value::as_str) != Some("completed"))
    {
        return Err("发布运行包含无效的 job 状态".into());
    }
    for required in &requirement.jobs {
        let matches = jobs
            .iter()
            .filter(|job| job.get("name").and_then(Value::as_str) == Some(required.as_str()))
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return Ok(None);
        }
        if matches.len() != 1 {
            return Err(format!("必需 job 重复：{required}").into());
        }
        let conclusion = matches[0].get("conclusion").and_then(Value::as_str);
        if conclusion != Some("success") {
            return Err(format!(
                "必需 job 未成功：{required} ({})",
                conclusion.unwrap_or("null")
            )
            .into());
        }
    }
    if jobs
        .iter()
        .any(|job| job.get("conclusion").and_then(Value::as_str) != Some("success"))
    {
        return Err("发布运行包含失败、取消、跳过或未完成的 job".into());
    }
    let job_receipts = jobs
        .iter()
        .map(|job| {
            json!({
                "id": job["id"],
                "name": job.get("name").cloned().unwrap_or(Value::Null),
            })
        })
        .collect::<Vec<_>>();
    Ok(Some(json!({
        "repository": requirement.repository,
        "workflow": requirement.workflow,
        "sha": requirement.sha,
        "run_id": run_id,
        "attempt": attempt,
        "jobs": job_receipts,
    })))
}

fn validate_job_identity(
    job: &Value,
    expected_sha: &str,
    run_id: u64,
    jobs_attempt: u64,
) -> Result<()> {
    if job.get("head_sha").and_then(Value::as_str) != Some(expected_sha)
        || job.get("run_id").and_then(Value::as_u64) != Some(run_id)
    {
        return Err("job 的源码或运行身份不匹配".into());
    }
    if job
        .get("run_attempt")
        .and_then(Value::as_u64)
        .is_some_and(|attempt| attempt != jobs_attempt)
    {
        return Err("job 返回的 attempt 与请求不一致".into());
    }
    positive_integer(job.get("id"), "job ID")?;
    Ok(())
}

fn job_is_pending(job: &Value) -> bool {
    job.get("status")
        .and_then(Value::as_str)
        .is_some_and(is_pending_status)
        || (job.get("status").and_then(Value::as_str) == Some("completed")
            && job.get("conclusion").is_none_or(Value::is_null))
}

fn is_pending_status(status: &str) -> bool {
    matches!(
        status,
        "queued" | "in_progress" | "waiting" | "pending" | "requested"
    )
}

fn positive_integer(value: Option<&Value>, label: &str) -> Result<u64> {
    value
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{label}必须是正整数").into())
}

pub(crate) fn validate_pair(
    receipt: &Value,
    evidence: &Value,
    backend_sha: &str,
    frontend_sha: &str,
) -> Result<()> {
    let sources = receipt.get("sources").and_then(Value::as_object);
    if receipt.get("format_version").and_then(Value::as_u64) != Some(1)
        || receipt.get("backend_sha").and_then(Value::as_str) != Some(backend_sha)
        || receipt.get("frontend_sha").and_then(Value::as_str) != Some(frontend_sha)
        || receipt.get("run_id") != evidence.get("run_id")
        || receipt.get("attempt") != evidence.get("attempt")
        || sources.is_none_or(|sources| {
            sources.keys().map(String::as_str).collect::<BTreeSet<_>>()
                != BTreeSet::from(["backend", "frontend"])
        })
        || !clean_source(
            sources.and_then(|sources| sources.get("backend")),
            backend_sha,
        )
        || !clean_source(
            sources.and_then(|sources| sources.get("frontend")),
            frontend_sha,
        )
    {
        return Err("真实全栈证据与本次源码组合或运行尝试不一致".into());
    }
    Ok(())
}

pub(crate) fn clean_source(value: Option<&Value>, expected_sha: &str) -> bool {
    let Some(Value::Object(source)) = value else {
        return false;
    };
    source.keys().map(String::as_str).collect::<BTreeSet<_>>()
        == BTreeSet::from(["files", "head", "patch_sha256"])
        && source.get("head").and_then(Value::as_str) == Some(expected_sha)
        && source.get("patch_sha256").and_then(Value::as_str) == Some(EMPTY_SHA256)
        && source
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
}

pub(crate) fn validate_fixture(
    receipt: Option<&Value>,
    backend_sha: &str,
    frontend_sha: &str,
    expected_fixture_sha256: &str,
) -> Result<()> {
    let Some(receipt) = receipt.and_then(Value::as_object) else {
        return Err("业务 crate 生成工作树收据缺失或未完成".into());
    };
    if receipt.get("format_version").and_then(Value::as_u64) != Some(1)
        || receipt.get("fixture").and_then(Value::as_str) != Some("business")
        || receipt.get("status").and_then(Value::as_str) != Some("ready")
    {
        return Err("业务 crate 生成工作树收据缺失或未完成".into());
    }
    let fixture_sha = receipt
        .get("fixture_sha256")
        .and_then(Value::as_str)
        .filter(|value| sha256_shape(Some(value)) && *value != EMPTY_SHA256);
    if fixture_sha != Some(expected_fixture_sha256) {
        return Err("业务 crate fixture 摘要与发布源码不一致".into());
    }
    let sources = receipt.get("sources").and_then(Value::as_object);
    let generated = receipt.get("generated").and_then(Value::as_object);
    if sources.is_none() || generated.is_none() {
        return Err("业务 crate 缺少源码或生成内容指纹".into());
    }
    let mut generated_files = BTreeMap::new();
    for (name, expected) in [("backend", backend_sha), ("frontend", frontend_sha)] {
        let source = sources.and_then(|sources| sources.get(name));
        if !clean_source(source, expected) {
            return Err("业务 crate 必须从本次精确 SHA 的干净源码生成".into());
        }
        let value = generated
            .and_then(|generated| generated.get(name))
            .and_then(Value::as_object)
            .ok_or_else(|| "业务 crate 缺少生成内容的源码指纹".to_string())?;
        if value.get("head").and_then(Value::as_str) != Some(expected)
            || !sha256_shape(value.get("patch_sha256").and_then(Value::as_str))
        {
            return Err("业务 crate 缺少生成内容的源码指纹".into());
        }
        let files = generated_file_map(value, name)?;
        if name == "frontend" {
            if Value::Object(value.clone()) != source.cloned().unwrap_or(Value::Null) {
                return Err("业务 crate 生成不得改写前端仓库源码".into());
            }
            generated_files.insert(name, files);
            continue;
        }
        if value.get("patch_sha256").and_then(Value::as_str) == Some(EMPTY_SHA256)
            && files.is_empty()
        {
            return Err(format!("业务 crate {name} 没有实际生成变化").into());
        }
        let missing = BUSINESS_BACKEND_OUTPUTS
            .iter()
            .filter(|path| !files.contains_key(**path))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!("业务 crate {name} 缺少必要生成输出：{missing:?}").into());
        }
        if BUSINESS_BACKEND_OUTPUTS.iter().any(|path| {
            files
                .get(*path)
                .is_some_and(|digest| digest == EMPTY_SHA256)
        }) {
            return Err(format!("业务 crate {name} 必要生成输出为空").into());
        }
        generated_files.insert(name, files);
    }
    if generated_files
        .get("backend")
        .and_then(|files| files.get("crates/order-business/src/resources/mod.rs"))
        .map(String::as_str)
        != Some(fixture_sha.unwrap_or_default())
    {
        return Err("业务 crate fixture 摘要与隔离工作树中的资源定义不一致".into());
    }
    Ok(())
}

fn generated_file_map(
    value: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<BTreeMap<String, String>> {
    let entries = value
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("业务 crate {name} 生成文件摘要不是列表"))?;
    let mut files = BTreeMap::new();
    for entry in entries {
        let Some(entry) = entry.as_object() else {
            return Err(format!("业务 crate {name} 生成文件摘要无效").into());
        };
        let path = entry.get("path").and_then(Value::as_str);
        let digest = entry.get("sha256").and_then(Value::as_str);
        if path.is_none_or(|path| {
            path.is_empty()
                || path.starts_with('/')
                || path.contains(['\\', '\0'])
                || path.split('/').any(|part| matches!(part, "" | "." | ".."))
        }) || !sha256_shape(digest)
            || path.is_some_and(|path| files.contains_key(path))
        {
            return Err(format!("业务 crate {name} 生成文件摘要无效或重复").into());
        }
        files.insert(
            path.unwrap_or_default().to_owned(),
            digest.unwrap_or_default().to_owned(),
        );
    }
    Ok(files)
}
