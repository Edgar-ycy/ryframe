//! 重启后核对同一代次的两份原事故证据；不生成正常关闭证明。
use super::*;

pub(in crate::recovery::fixture_control::services) const VERSION: u64 = 3;

pub(super) fn observe_registered(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    previous: Option<&Value>,
) -> Result<Value> {
    #[cfg(windows)]
    if let Some(observation) = windows::observe(runtime, context, services, previous)? {
        return Ok(observation);
    }
    #[cfg(not(windows))]
    let _ = (context, previous);
    observe(runtime, services)
}

pub(super) fn recorded(context: &Context, history: &Value) -> Result<Option<Value>> {
    if history["external_recovery"].is_null() {
        return Ok(None);
    }
    let recovery = history::evidence::bound(&context.run, &history["external_recovery"])?;
    let proof = history::evidence::bound(&context.run, &recovery["evidence"])?;
    Ok(Some(json!({"termination":proof["observation"]})))
}

pub(super) fn check_registered(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    expected: &Value,
    message: &'static str,
) -> Result<()> {
    check(
        observe_registered(runtime, context, services, Some(expected))? == *expected,
        message,
    )
}

pub(in crate::recovery::fixture_control::services) fn verify_historical(
    run: &Path,
    rustfs: &Path,
    redis: &Path,
    observation: &Value,
) -> Result<()> {
    #[cfg(windows)]
    {
        windows::verify_historical(run, rustfs, redis, observation)
    }
    #[cfg(not(windows))]
    {
        let _ = (run, rustfs, redis, observation);
        Err("Windows 服务事故收据须在原平台核对".into())
    }
}

#[cfg(windows)]
#[path = "incident/windows.rs"]
mod windows;
