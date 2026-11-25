//! 核对登记服务的原事故证据并保留正常关闭证明的原语义。
use super::*;

pub(in crate::recovery::fixture_control::services) const VERSION: u64 = 3;
pub(in crate::recovery::fixture_control::services) const MIXED_VERSION: u64 = 4;
pub(super) const MIXED_KIND: &str = "windows-mixed-service-termination";

pub(super) fn proof_version(evidence: &Value) -> u64 {
    if evidence["kind"] == MIXED_KIND {
        MIXED_VERSION
    } else if evidence.get("host_reboot").is_some() {
        VERSION
    } else if evidence.get("redis_external").is_some() {
        2
    } else {
        1
    }
}

pub(super) fn observe_registered(
    runtime: &mut dyn Runtime,
    context: &Context,
    services: &Value,
    previous: Option<&Value>,
) -> Result<Value> {
    #[cfg(windows)]
    {
        if let Some(recorded) =
            previous.filter(|value| value["termination"]["evidence"]["kind"] == MIXED_KIND)
        {
            let observed = mixed::observe(runtime, context, services, Some(recorded))?
                .ok_or("原混合终态已无法按同一事故类型核对")?;
            check(
                recorded["termination"] == observed["termination"],
                "原混合终态与已核对的完整事故证据不同",
            )?;
            return Ok(observed);
        }
        if let Some(observation) = windows::observe(runtime, context, services, previous)? {
            return Ok(observation);
        }
        if let Some(observation) = mixed::observe(runtime, context, services, None)? {
            check(previous.is_none(), "不能以混合终态替换已绑定的原服务状态")?;
            return Ok(observation);
        }
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

pub(in crate::recovery::fixture_control::services) fn verify_mixed_historical(
    run: &Path,
    rustfs: &Path,
    redis: &Path,
    registered: &Value,
    observation: &Value,
) -> Result<()> {
    #[cfg(windows)]
    {
        mixed::verify_historical(run, rustfs, redis, registered, observation)
    }
    #[cfg(not(windows))]
    {
        let _ = (run, rustfs, redis, registered, observation);
        Err("Windows 服务事故收据须在原平台核对".into())
    }
}

#[cfg(windows)]
#[path = "incident/mixed.rs"]
mod mixed;
#[cfg(windows)]
#[path = "incident/windows.rs"]
mod windows;

#[test]
fn external_proof_versions_keep_prior_forms_and_assign_mixed_version_four() {
    assert_eq!(proof_version(&json!({"rustfs_tree":{}})), 1);
    assert_eq!(proof_version(&json!({"redis_external":{}})), 2);
    assert_eq!(proof_version(&json!({"host_reboot":{}})), 3);
    assert_eq!(proof_version(&json!({"kind":MIXED_KIND,"host_boot":{}})), 4);
}
