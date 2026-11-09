//! seed 业务关闭始终使用原运行交接和已登记共享控制库。
use super::seed_capacity::Guard;
use crate::{
    Result,
    recovery::clone::native::{
        resources::common,
        seed::{self, capacity, runtime_model, source_registration::identity},
    },
};
use serde_json::Value;

pub(super) fn execute(guard: &Guard<'_, '_>, request: &Value) -> Result<Value> {
    let context = guard.live.context;
    let backend = context.backend;
    let run = context.run;
    let number = context.number;
    let inputs = runtime_model::inputs(backend, run)?;
    let binding = common::binding(&inputs.runtime.join("handoff.json"))?;
    let fixed = || {
        if common::read_bound(backend, &binding, "seed 关闭交接")? != inputs.handoff
            || identity::verified(backend, run, request, Some(number))?
                != inputs.handoff["identity"]
            || capacity::evidence(backend, run, request, Some(number))?
                != inputs.handoff["capacity"]
        {
            return Err("seed 关闭期间原交接、身份或容量证明变化".into());
        }
        let schedules = guard.full()?;
        if inputs.handoff["schedules"].as_array() != Some(&schedules) {
            return Err("seed 关闭期间原调度证据变化".into());
        }
        common::bound_file(backend, &binding, "seed 关闭交接后像")?;
        Ok(())
    };
    seed::close_runtime(
        backend,
        run,
        number,
        &binding,
        &guard.live.target(),
        &fixed,
        &|| {
            context.owner.check_initialization_lock()?;
            guard.live.checkpoint()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::clone::native::target_verify::guard::Generation;
    use std::fs;

    #[test]
    fn close_drain_checks_actual_initialization_lock_members() -> Result<()> {
        let backend = crate::workspace::root_dir();
        let local = backend.join(".local-tests");
        fs::create_dir_all(&local)?;
        let fixture = tempfile::tempdir_in(local)?;
        let target = common::local_directory(&backend, fixture.path(), "排空锁夹具")?;
        let generation = Generation::acquire(&backend, &target)?;
        generation.check_initialization_lock()?;
        let unexpected = target.join("initialize.lock/unknown.json");
        fs::write(&unexpected, b"fixture")?;
        assert!(generation.check_initialization_lock().is_err());
        assert_eq!(fs::read(&unexpected)?, b"fixture");
        fs::remove_file(unexpected)?;
        generation.finish(Ok(()))
    }
}
