//! 一次性身份生产者只接受原控制器授权，关闭 stdin 后读取完整 stdout。

use super::{Context, Kind, Producer, SeedLaunch, document, parse};
use crate::Result;
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(7200);

pub(in crate::recovery::clone::native) fn with_identity(
    context: &Context<'_>,
    request: &Value,
    registration: &Value,
) -> Result<Value> {
    let kind = kind(context.mode)?;
    let launch = SeedLaunch::load(context, request, registration)?;
    let mut producer = Producer::start(context, kind, Some(launch))?;
    let result = (|| {
        let deadline = Instant::now() + TIMEOUT;
        let grant = start_grant(
            context.run,
            context.number,
            &producer.owner,
            &producer.launch,
            &producer.process,
        );
        // 子进程读取到 EOF 才核验启动授权。stdout EOF 后还须确认原进程成功退出。
        let raw = producer.exchange(&grant, true, deadline)?;
        if !producer.job.wait(deadline)?.success() {
            return Err("原生身份生产者未成功退出；保留原意图与独立日志，禁止重放".into());
        }
        checked_stdout(&raw)
    })();
    producer.finish(result, "identity-cleanup", None)
}

pub(super) fn kind(mode: &str) -> Result<Kind> {
    match mode {
        "identities-apply" => Ok(Kind::Identity("identity-apply")),
        "identities-verify" => Ok(Kind::Identity("identity-verify")),
        _ => Err("身份生产者只接受当前正式应用或验证阶段".into()),
    }
}

pub(super) fn start_grant(
    run: &Path,
    attempt: u64,
    owner: &Value,
    launch: &Value,
    process: &Value,
) -> Value {
    json!({"operation":"start","run_dir":run,"attempt":attempt,
        "identity_lock_owner":owner,"launch":launch,"process":process})
}

pub(super) fn checked_stdout(raw: &[u8]) -> Result<Value> {
    let value = parse(raw, "身份生产者结果")?;
    document::object(&value, "身份生产者结果")?;
    Ok(value)
}
