//! 从精确失败启动的完整前像接续，只采一次实时后像。
use super::*;
use crate::recovery::source::evidence::running::history::prelaunch::revalidated::Original;

pub(in crate::recovery::source::evidence::control) fn preflight(
    backend: &Path,
    run: &Path,
    prefix: &[Value],
    number: u64,
    request: &Path,
) -> Result<()> {
    let descriptor = common::binding(request)?;
    let archive = prelaunch::closed(backend, run, prefix)?;
    revalidated::load(backend, run, prefix, number, &descriptor, &archive)?;
    Ok(())
}

pub(in crate::recovery::source::evidence::control) fn run<T: Transport + Sync>(
    frame: &mut Frame<'_>,
    request: &Path,
    transport: &T,
) -> Result<Value> {
    let descriptor = common::binding(request)?;
    let archive = prelaunch::closed(frame.backend, frame.run, &frame.prefix)?;
    let original = load_original(frame, &descriptor, &archive)?;
    let prepared = prepare_observation(
        frame.backend,
        frame.run,
        frame.state(),
        frame.number,
        &descriptor,
    )?;
    let config = prepared.config.clone();
    #[cfg(windows)]
    frame.hold_sources(&prepared.facts["request"])?;
    let observation = Observation::create(
        frame,
        prepared,
        configuration::Selection::Copy(&config),
        transport,
    )?;
    observation.quiet()?;
    let (after, current) = observation.capture_revalidated_after()?;
    if current["image"] != original.image {
        return Err("同一正式来源的完整前后像不同；保留原件和新后像，不发布或重放".into());
    }
    let current_original = load_original(frame, &descriptor, &archive)?;
    if original.start != current_original.start
        || original.proof != current_original.proof
        || original.before != current_original.before
        || original.image != current_original.image
        || common::binding(request)? != descriptor
    {
        return Err("来源完整后像采集期间原失败、前像或正式请求变化".into());
    }
    observation.quiet()?;
    let evidence = FailureEvidence {
        start: original.start,
        proof: original.proof,
        collection: Value::Null,
    };
    receipt(
        &observation,
        revalidated::STATUS,
        &evidence,
        &original.before,
        &after,
        true,
    )
}

fn load_original(frame: &Frame<'_>, request: &Value, archive: &Value) -> Result<Original> {
    revalidated::load(
        frame.backend,
        frame.run,
        &frame.prefix,
        frame.number,
        request,
        archive,
    )
}
