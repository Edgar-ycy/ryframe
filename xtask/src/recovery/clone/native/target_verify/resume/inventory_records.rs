//! 库存重试只接受完整只读失败或唯一成功；拒绝挂起、重叠与未知写入。
use super::*;
use std::collections::BTreeMap;

pub(super) const INTENT: &[&str] = &[
    "format_version",
    "kind",
    "attempt",
    "at",
    "failure",
    "prepared_files",
    "remote_writes",
];
const FAILURE: &[&str] = &[
    "format_version",
    "kind",
    "attempt",
    "at",
    "intent",
    "source_failure",
    "prepared_files",
    "error_type",
    "remote_writes",
];

pub(super) struct Context<'a> {
    pub(super) backend: &'a Path,
    pub(super) target: &'a Path,
    pub(super) failure: &'a Value,
    pub(super) failure_at: &'a Value,
    pub(super) prepared_files: &'a Value,
    pub(super) plan: &'a Value,
    pub(super) executable: &'a str,
    // 父级已核验的 reset 执行请求；本层只核验库存记录，不选择配置合同。
    pub(super) request: &'a Value,
    pub(super) available: &'a BTreeSet<String>,
}

pub(super) fn execute(
    backend: &Path,
    target: &Path,
    value: &Value,
    success: &Value,
) -> Result<Value> {
    json_io::exact(
        value,
        &[
            "failure",
            "source_failure_at",
            "prepared_files",
            "plan",
            "executable",
            "request",
            "available",
        ],
        "库存重试上下文",
    )?;
    let available = json_io::array(&value["available"], "库存证据集合")?
        .iter()
        .map(|value| Ok(json_io::text(value, "库存证据路径")?.to_owned()))
        .collect::<Result<BTreeSet<_>>>()?;
    let context = Context {
        backend,
        target,
        failure: &value["failure"],
        failure_at: &value["source_failure_at"],
        prepared_files: &value["prepared_files"],
        plan: &value["plan"],
        executable: json_io::text(&value["executable"], "reset 程序")?,
        request: &value["request"],
        available: &available,
    };
    let success = if success.is_null() {
        None
    } else {
        json_io::exact(success, &["attempt", "at"], "库存成功重试")?;
        Some((attempt(&success["attempt"])?, &success["at"]))
    };
    let (count, files) = records(&context, success)?;
    Ok(json!({"attempts":count,"files":files}))
}

pub(super) fn records(
    context: &Context<'_>,
    success: Option<(&str, &Value)>,
) -> Result<(usize, BTreeSet<String>)> {
    let Context {
        backend,
        target,
        failure,
        failure_at,
        prepared_files,
        ..
    } = context;
    let mut intents = BTreeMap::new();
    let mut failures = BTreeMap::new();
    let mut confirmations = BTreeSet::new();
    let pattern =
        regex::Regex::new(r"^resume-initialize-([a-f0-9]{32})\.(intent|failure|confirmed)\.json$")?;
    for name in names(target, r"^resume-initialize-.*\.json$")? {
        let parts = pattern
            .captures(&name)
            .ok_or("库存续作包含未知或无法归属的阶段证据")?;
        let attempt = parts[1].to_owned();
        if &parts[2] == "confirmed" {
            confirmations.insert(attempt);
            continue;
        }
        let path = target.join(&name);
        let value = read(backend, &path)?;
        if &parts[2] == "intent" {
            json_io::exact(&value, INTENT, "库存续作 intent")?;
            check(
                value["format_version"].as_u64() == Some(1)
                    && value["kind"] == "devex-clone-initialize-resume-intent"
                    && value["attempt"] == attempt
                    && timestamp(&value["at"]).is_ok()
                    && value["failure"] == **failure
                    && value["prepared_files"] == **prepared_files
                    && zero(&value["remote_writes"]),
                "库存续作 intent 没有绑定原只读失败",
            )?;
            intents.insert(attempt, (binding(backend, &path)?, value["at"].clone()));
        } else {
            json_io::exact(&value, FAILURE, "库存续作失败")?;
            check(
                value["format_version"].as_u64() == Some(1)
                    && value["kind"] == "devex-clone-initialize-resume-failure"
                    && value["attempt"] == attempt
                    && timestamp(&value["at"]).is_ok()
                    && value["source_failure"] == **failure
                    && value["prepared_files"] == **prepared_files
                    && value["error_type"]
                        .as_str()
                        .is_some_and(|error| !error.is_empty())
                    && zero(&value["remote_writes"]),
                "库存续作失败收据不是只读失败",
            )?;
            failures.insert(attempt, value);
        }
    }
    let expected = success
        .map(|(attempt, _)| attempt.to_owned())
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut terminals = failures.keys().cloned().collect::<BTreeSet<_>>();
    terminals.extend(expected.iter().cloned());
    check(
        confirmations == expected
            && failures.keys().all(|attempt| !expected.contains(attempt))
            && intents.keys().cloned().collect::<BTreeSet<_>>() == terminals,
        "库存续作包含挂起、重复或未知结果",
    )?;
    let mut intervals = Vec::new();
    for (attempt, value) in &failures {
        let (intent, at) = &intents[attempt];
        check(
            value["intent"] == *intent && ordered(&[failure_at, at, &value["at"]]),
            "库存续作失败收据不属于原 intent",
        )?;
        intervals.push((timestamp(at)?, timestamp(&value["at"])?));
    }
    intervals.sort();
    check(
        intervals.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "库存续作失败尝试时间谱系重叠",
    )?;
    if let Some((attempt, terminal)) = success {
        check(
            ordered(&[failure_at, &intents[attempt].1, terminal]),
            "库存续作成功尝试时间谱系无效",
        )?;
        check(
            intervals.last().is_none_or(|interval| {
                timestamp(&intents[attempt].1).is_ok_and(|at| interval.1 <= at)
            }),
            "库存续作成功尝试早于既有失败尝试",
        )?;
    }
    let mut files = BTreeSet::new();
    for attempt in intents.keys() {
        files.insert(format!("resume-initialize-{attempt}.intent.json"));
        files.insert(format!(
            "resume-initialize-{attempt}.{}.json",
            if confirmations.contains(attempt) {
                "confirmed"
            } else {
                "failure"
            }
        ));
        files.extend(retry_plan(context, attempt)?);
        if failures.contains_key(attempt) {
            files.extend(super::inventory_failure::files(
                backend,
                target,
                &format!("inventory-resume-{attempt}"),
                context.request,
                context.available,
            )?);
        }
    }
    Ok((intents.len(), files))
}

fn retry_plan(context: &Context<'_>, attempt: &str) -> Result<BTreeSet<String>> {
    let name = format!("resume-reset-plan-{attempt}.json");
    check(
        read(context.backend, &context.target.join(&name))? == *context.plan,
        "库存续作 reset plan 不属于原固定计划",
    )?;
    let pattern = format!(r"^resume-reset-plan-{attempt}-[a-f0-9]{{32}}\.command\.json$");
    let commands = names(context.target, &pattern)?;
    check(
        commands.len() == 1,
        "库存续作必须保留唯一 reset plan 命令收据",
    )?;
    let command = commands
        .first()
        .ok_or("库存续作必须保留唯一 reset plan 命令收据")?;
    super::super::reset::plan_receipt(
        context.backend,
        context.target,
        &context.target.join(command),
        context.executable,
        context.plan,
        context.request,
    )?;
    Ok(BTreeSet::from([name, command.clone()]))
}
