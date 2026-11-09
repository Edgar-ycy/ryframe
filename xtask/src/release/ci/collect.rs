use std::{
    collections::{BTreeSet, HashSet},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::Result;

use super::super::{
    evidence::{Requirement, latest_run, validate_fixture, validate_pair, validate_run},
    github::Api,
    support::{commit_shape, sha256_shape},
};

use super::artifacts;

pub(crate) fn coordinated_evidence(
    deadline: Instant,
    required: &[Requirement],
    frontend_sha: &str,
    fixture_sha256: &str,
    tag_oids: Option<&(String, String)>,
    tag_requirements: Option<&[Requirement]>,
    api: &dyn Api,
) -> Result<Value> {
    coordinated_evidence_with(
        deadline,
        required,
        frontend_sha,
        fixture_sha256,
        tag_oids,
        tag_requirements,
        |requirement| collect(api, requirement, deadline),
        |evidence, backend_sha, frontend_sha, fixture_sha256| {
            artifacts::pair_receipts(
                api,
                evidence,
                backend_sha,
                frontend_sha,
                fixture_sha256,
                deadline,
            )
        },
        Instant::now,
        thread::sleep,
        |requirements, oids| validate_remote_tags(api, requirements, oids, deadline),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn coordinated_evidence_with<C, P, N, S, T>(
    deadline: Instant,
    required: &[Requirement],
    frontend_sha: &str,
    fixture_sha256: &str,
    tag_oids: Option<&(String, String)>,
    tag_requirements: Option<&[Requirement]>,
    mut collect_run: C,
    mut read_pairs: P,
    mut now: N,
    mut sleep: S,
    mut read_tags: T,
) -> Result<Value>
where
    C: FnMut(&Requirement) -> Result<Option<Value>>,
    P: FnMut(&Value, &str, &str, &str) -> Result<Value>,
    N: FnMut() -> Instant,
    S: FnMut(Duration),
    T: FnMut(&[Requirement], &(String, String)) -> Result<Value>,
{
    if !commit_shape(Some(frontend_sha)) {
        return Err("缺少有效的前端发布提交 SHA".into());
    }
    if !sha256_shape(Some(fixture_sha256)) {
        return Err("缺少发布源码中的业务 crate fixture 摘要".into());
    }
    loop {
        let evidence =
            await_evidence_with(required, deadline, &mut collect_run, &mut now, &mut sleep)?;
        let (extended, selected) = extended_evidence(required, &evidence)?;
        let backend_sha = &extended.sha;
        let receipts = read_pairs(selected, backend_sha, frontend_sha, fixture_sha256)?;
        let Some(receipts_object) = receipts.as_object() else {
            return Err("全栈源码组合收据必须是 JSON 对象".into());
        };
        if receipts_object
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != BTreeSet::from(["business", "core"])
        {
            return Err("全栈证据必须同时包含 core 和 business".into());
        }
        for receipt in receipts_object.values() {
            validate_pair(receipt, selected, backend_sha, frontend_sha)?;
        }
        validate_fixture(
            receipts_object["business"].get("fixture"),
            backend_sha,
            frontend_sha,
            fixture_sha256,
        )?;
        let rechecked = required
            .iter()
            .map(&mut collect_run)
            .collect::<Result<Vec<_>>>()?;
        if rechecked
            .iter()
            .zip(&evidence)
            .all(|(current, selected)| current.as_ref() == Some(selected))
        {
            let mut result = json!({"runs": evidence, "source_pairs": receipts});
            if let Some(tag_oids) = tag_oids {
                let requirements = tag_requirements
                    .filter(|requirements| requirements.len() == 2)
                    .ok_or_else(|| "远端 tag 核验必须分别绑定后端和前端仓库".to_string())?;
                result["remote_tags"] = read_tags(requirements, tag_oids)?;
            }
            return Ok(result);
        }
        let remaining = deadline.saturating_duration_since((now)());
        if remaining.is_zero() {
            return Err("源码组合复核期间 CI 发生变化，等待超时".into());
        }
        sleep(remaining.min(Duration::from_secs(15)));
    }
}

fn extended_evidence<'a>(
    required: &'a [Requirement],
    evidence: &'a [Value],
) -> Result<(&'a Requirement, &'a Value)> {
    if evidence.len() < 2 || evidence.len() != required.len() {
        return Err("发布 CI 声明与证据必须完整对应，且至少包含两个运行".into());
    }
    let mut extended = None;
    for (requirement, receipt) in required.iter().zip(evidence) {
        if receipt.get("repository").and_then(Value::as_str)
            != Some(requirement.repository.as_str())
            || receipt.get("workflow").and_then(Value::as_str)
                != Some(requirement.workflow.as_str())
            || receipt.get("sha").and_then(Value::as_str) != Some(requirement.sha.as_str())
        {
            return Err("发布 CI 证据与对应声明的仓库、工作流或源码 SHA 不一致".into());
        }
        if requirement.workflow == "extended-ci.yml" {
            if extended.is_some() {
                return Err("发布 CI 声明包含重复的 extended-ci.yml，无法选择全栈收据".into());
            }
            extended = Some((requirement, receipt));
        }
    }
    extended.ok_or_else(|| "发布 CI 声明缺少 extended-ci.yml，无法读取全栈收据".into())
}

pub(crate) fn await_evidence_with<C, N, S>(
    required: &[Requirement],
    deadline: Instant,
    mut collect_run: C,
    mut now: N,
    mut sleep: S,
) -> Result<Vec<Value>>
where
    C: FnMut(&Requirement) -> Result<Option<Value>>,
    N: FnMut() -> Instant,
    S: FnMut(Duration),
{
    loop {
        let evidence = required
            .iter()
            .map(&mut collect_run)
            .collect::<Result<Vec<_>>>()?;
        if evidence.iter().all(Option::is_some) {
            return Ok(evidence
                .into_iter()
                .map(|value| value.expect("已确认所有证据存在"))
                .collect());
        }
        let remaining = deadline.saturating_duration_since((now)());
        if remaining.is_zero() {
            return Err("等待 CI 超时：仍有缺失或未完成的运行".into());
        }
        sleep(remaining.min(Duration::from_secs(15)));
    }
}

pub(crate) fn collect(
    api: &dyn Api,
    requirement: &Requirement,
    deadline: Instant,
) -> Result<Option<Value>> {
    let prefix = format!("repos/{}/actions", requirement.repository);
    let runs = pages(
        api,
        &format!(
            "{prefix}/workflows/{}/runs?head_sha={}",
            requirement.workflow, requirement.sha
        ),
        "workflow_runs",
        deadline,
    )?;
    let Some(selected) = latest_run(&runs, requirement).cloned() else {
        return Ok(None);
    };
    let run_id = selected
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "运行缺少有效 ID".to_string())?;
    let run = api.get(
        &format!("{prefix}/runs/{run_id}"),
        api_timeout(deadline, Instant::now())?,
    )?;
    if run.get("id").and_then(Value::as_u64) != Some(run_id) {
        return Err("运行详情与已选择的 run ID 不一致".into());
    }
    let mut jobs = Vec::new();
    if run.get("status").and_then(Value::as_str) == Some("completed") {
        let attempt = run
            .get("run_attempt")
            .and_then(Value::as_u64)
            .ok_or_else(|| "运行缺少有效 attempt".to_string())?;
        jobs = pages(
            api,
            &format!("{prefix}/runs/{run_id}/attempts/{attempt}/jobs"),
            "jobs",
            deadline,
        )?;
    }
    let current = api.get(
        &format!("{prefix}/runs/{run_id}"),
        api_timeout(deadline, Instant::now())?,
    )?;
    let newest_runs = pages(
        api,
        &format!(
            "{prefix}/workflows/{}/runs?head_sha={}",
            requirement.workflow, requirement.sha
        ),
        "workflow_runs",
        deadline,
    )?;
    let newest = latest_run(&newest_runs, requirement);
    if newest.is_none()
        || run_identity(&current) != run_identity(&run)
        || newest.is_some_and(|newest| run_identity(newest) != run_identity(&run))
    {
        return Ok(None);
    }
    let attempt = run
        .get("run_attempt")
        .and_then(Value::as_u64)
        .ok_or_else(|| "运行缺少有效 attempt".to_string())?;
    validate_run(&run, &jobs, requirement, attempt)
}

fn run_identity(value: &Value) -> [Value; 5] {
    [
        value.get("id").cloned().unwrap_or(Value::Null),
        value.get("run_attempt").cloned().unwrap_or(Value::Null),
        value.get("head_sha").cloned().unwrap_or(Value::Null),
        value.get("status").cloned().unwrap_or(Value::Null),
        value.get("conclusion").cloned().unwrap_or(Value::Null),
    ]
}

pub(crate) fn pages(
    api: &dyn Api,
    endpoint: &str,
    field: &str,
    deadline: Instant,
) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    for page in 1..=100 {
        let separator = if endpoint.contains('?') { '&' } else { '?' };
        let url = format!("{endpoint}{separator}per_page=100&page={page}");
        let response = api.get(&url, api_timeout(deadline, Instant::now())?)?;
        let items = response
            .get(field)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("GitHub API 响应缺少数组字段 {field}"))?;
        result.extend(items.iter().cloned());
        if items.len() < 100 {
            return Ok(result);
        }
    }
    Err("证据分页超出安全上限，拒绝截断后判定成功".into())
}

pub(crate) fn api_timeout(deadline: Instant, now: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        return Err("发布 CI 证据采集超过总截止时间".into());
    }
    Ok(remaining.min(Duration::from_secs(60)))
}

pub(crate) fn validate_remote_tags(
    api: &dyn Api,
    required: &[Requirement],
    tag_oids: &(String, String),
    deadline: Instant,
) -> Result<Value> {
    if required.len() < 2
        || required[0].tag.is_none()
        || required[1].tag.is_none()
        || !commit_shape(Some(&tag_oids.0))
        || !commit_shape(Some(&tag_oids.1))
    {
        return Err("远端 tag 核验缺少精确的双方身份".into());
    }
    let targets = [(&required[0], &tag_oids.0), (&required[1], &tag_oids.1)];
    for (requirement, oid) in targets {
        remote_tag_ref(api, requirement, oid, deadline)?;
    }
    let mut result = Vec::new();
    for (requirement, oid) in targets {
        let commit = remote_tag_commit(api, requirement, oid, deadline)?;
        if commit != requirement.sha {
            return Err(format!(
                "远端 annotated tag 解引用提交错误：{}",
                requirement.repository
            )
            .into());
        }
        result.push(json!({
            "repository": requirement.repository,
            "tag": requirement.tag,
            "tag_oid": oid,
            "commit": commit,
        }));
    }
    for (requirement, oid) in targets {
        remote_tag_ref(api, requirement, oid, deadline)?;
    }
    Ok(Value::Array(result))
}

fn remote_tag_ref(
    api: &dyn Api,
    requirement: &Requirement,
    expected_oid: &str,
    deadline: Instant,
) -> Result<()> {
    let tag = requirement
        .tag
        .as_deref()
        .ok_or_else(|| "远端 tag 核验缺少 tag".to_string())?;
    let endpoint = format!("repos/{}/git/ref/tags/{tag}", requirement.repository);
    let reference = api.get(&endpoint, api_timeout(deadline, Instant::now())?)?;
    let target = reference.get("object");
    if reference.get("ref").and_then(Value::as_str) != Some(&format!("refs/tags/{tag}"))
        || target
            .and_then(|target| target.get("type"))
            .and_then(Value::as_str)
            != Some("tag")
        || target
            .and_then(|target| target.get("sha"))
            .and_then(Value::as_str)
            != Some(expected_oid)
    {
        return Err(format!(
            "远端 annotated tag 已移动或对象身份错误：{}",
            requirement.repository
        )
        .into());
    }
    Ok(())
}

fn remote_tag_commit(
    api: &dyn Api,
    requirement: &Requirement,
    tag_oid: &str,
    deadline: Instant,
) -> Result<String> {
    let mut current = tag_oid.to_owned();
    let mut visited = HashSet::new();
    for _ in 0..8 {
        if !visited.insert(current.clone()) {
            return Err(format!("远端 annotated tag 出现循环：{}", requirement.repository).into());
        }
        let value = api.get(
            &format!("repos/{}/git/tags/{current}", requirement.repository),
            api_timeout(deadline, Instant::now())?,
        )?;
        let object = value.get("object");
        if value.get("sha").and_then(Value::as_str) != Some(&current) || object.is_none() {
            return Err(format!(
                "远端 annotated tag 对象响应无效：{}",
                requirement.repository
            )
            .into());
        }
        let kind = object
            .and_then(|object| object.get("type"))
            .and_then(Value::as_str);
        let target = object
            .and_then(|object| object.get("sha"))
            .and_then(Value::as_str);
        if !commit_shape(target) {
            return Err(format!("远端 annotated tag 目标无效：{}", requirement.repository).into());
        }
        match kind {
            Some("commit") => return Ok(target.unwrap_or_default().to_owned()),
            Some("tag") => current = target.unwrap_or_default().to_owned(),
            _ => {
                return Err(
                    format!("远端 annotated tag 未指向提交：{}", requirement.repository).into(),
                );
            }
        }
    }
    Err(format!(
        "远端 annotated tag 解引用层数超限：{}",
        requirement.repository
    )
    .into())
}
