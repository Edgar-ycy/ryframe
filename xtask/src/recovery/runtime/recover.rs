//! 回收中断的运行控制器，再在同一内核 guard 下接续当代停止核验。
use std::{collections::HashSet, fs, path::Path};

use serde_json::{Value, json};

use super::{
    bind::ownership::{Owner, control_root},
    dataset_authority::{inputs::Inputs, reads::Window},
    generation::STATE,
    registration::backend_root,
    status::{Bound, optional_file},
    stop,
};
use crate::{
    Result,
    cli::{RuntimeGenerationOptions, RuntimeRecoverOptions},
    recovery::{
        clone::native::resources::common,
        inputs::document::{self, Document},
    },
};

#[path = "recover/creation.rs"]
mod creation;
#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime/recover.rs"]
mod tests;

use creation::Creation;

pub(super) fn run(root: &Path, options: &RuntimeRecoverOptions) -> Result<Value> {
    let backend = backend_root(root)?;
    let generation = &options.generation;
    let bound = read_binding(&backend, generation)?;
    let control = control_root(&backend)?;
    let operations = Allowed::read(&bound, generation.generation)?;
    Owner::recover_with(
        &control,
        &operations.recover,
        &options.owner,
        &operations.values,
        |previous| {
            prepare(
                &backend,
                generation,
                &bound,
                &control,
                &operations,
                previous,
            )
        },
        |owner, recovery, (before, mut inputs)| {
            let result = continue_locked(&backend, generation, &bound, owner, before, &mut inputs);
            inputs
                .finish(result)
                .map(|stopped| reply(stopped, recovery))
        },
    )
}

fn read_binding(backend: &Path, options: &RuntimeGenerationOptions) -> Result<Bound> {
    let window = Window::direct()?;
    let mut inputs = Inputs::new(backend);
    let bound = Bound::read(backend, &options.control, &mut inputs);
    window.finish(inputs.finish(bound))
}

struct Allowed {
    start: String,
    recover: String,
    values: Vec<String>,
}

impl Allowed {
    fn read(bound: &Bound, generation: u32) -> Result<Self> {
        let start = bound.operation("start")?;
        let recover = bound.operation("recover")?;
        let values = vec![
            start.clone(),
            bound.operation("status")?,
            bound.operation("stop")?,
            recover.clone(),
            format!("{}:{generation}", bound.operation("bind")?),
        ];
        Ok(Self {
            start,
            recover,
            values,
        })
    }

    fn first_start(&self, control: &Path, generation: u32, previous: &Value) -> Result<bool> {
        if generation != 1 {
            return Ok(false);
        }
        let mut current = previous.clone();
        let mut seen = HashSet::new();
        while current["operation"] == self.recover {
            let token = document::text(&current["token"], "恢复控制器接续令牌")?;
            if !seen.insert(token.to_owned()) {
                return Err("首代运行控制器接续收据形成循环，保留现场".into());
            }
            current = Owner::prior_recovered_owner(control, &current)?;
        }
        Ok(current["operation"] == self.start)
    }
}

fn prepare(
    backend: &Path,
    options: &RuntimeGenerationOptions,
    bound: &Bound,
    control: &Path,
    operations: &Allowed,
    previous: &Value,
) -> Result<(Before, Inputs)> {
    let window = Window::direct()?;
    let mut inputs = Inputs::new(backend);
    let before = (|| {
        if &Bound::read(backend, &options.control, &mut inputs)? != bound {
            return Err("回收原恢复控制器前登记或目标计划发生变化".into());
        }
        Before::read(
            backend,
            bound,
            options.generation,
            || operations.first_start(control, options.generation, previous),
            &mut inputs,
        )
    })();
    let before = window.finish(inputs.finish(before))?;
    Ok((before, inputs))
}

enum Before {
    Existing(Existing),
    First(Creation),
}

impl Before {
    fn read(
        backend: &Path,
        bound: &Bound,
        generation: u32,
        first_start: impl FnOnce() -> Result<bool>,
        inputs: &mut Inputs,
    ) -> Result<Self> {
        let state_path = bound.runtime.join(STATE);
        if optional_file(&state_path)? {
            return Existing::read(backend, bound, generation, inputs, &state_path)
                .map(Self::Existing);
        }
        if !first_start()? {
            return Err("无生命周期状态时只能恢复首代启动控制器".into());
        }
        Creation::read(backend, bound, inputs).map(Self::First)
    }
}

struct Existing {
    state: Document,
    identity: (u64, u64),
    runtime_identity: (u64, u64),
}

impl Existing {
    fn read(
        backend: &Path,
        bound: &Bound,
        generation: u32,
        inputs: &mut Inputs,
        path: &Path,
    ) -> Result<Self> {
        let runtime_identity = directory_identity(&bound.runtime)?;
        let identity = common::device_inode_for(path, &fs::symlink_metadata(path)?)?;
        let state = document::read(path, backend, "恢复运行生命周期")?;
        bound.validate_generation(&state.value, inputs)?;
        let current = state.value["generations"]
            .as_array()
            .and_then(|rows| rows.last())
            .ok_or("恢复运行生命周期缺少当代记录")?;
        if current["number"].as_u64() != Some(u64::from(generation)) {
            return Err("控制恢复必须绑定当前明确运行代次".into());
        }
        bound.check_stop_authority(&current["request"]["authority"])?;
        let existing = Self {
            state,
            identity,
            runtime_identity,
        };
        existing.check(&bound.runtime)?;
        Ok(existing)
    }

    fn check(&self, runtime: &Path) -> Result<()> {
        self.state.assert_unchanged()?;
        let path = &self.state.path;
        if common::device_inode_for(path, &fs::symlink_metadata(path)?)? != self.identity
            || directory_identity(runtime)? != self.runtime_identity
        {
            return Err("回收原控制器期间运行目录或生命周期文件身份变化".into());
        }
        Ok(())
    }
}

fn directory_identity(path: &Path) -> Result<(u64, u64)> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || crate::recovery::clone::native::digest::linked(&metadata) {
        return Err("恢复运行目录必须保持原普通目录身份".into());
    }
    common::device_inode_for(path, &metadata)
}

fn continue_locked(
    backend: &Path,
    options: &RuntimeGenerationOptions,
    bound: &Bound,
    owner: &Owner,
    before: Before,
    inputs: &mut Inputs,
) -> Result<Option<Value>> {
    let window = Window::begin()?;
    let current = Bound::read(backend, &options.control, inputs);
    let checked = current.and_then(|current| {
        if &current != bound {
            return Err("接管恢复控制器后登记或目标计划发生变化".into());
        }
        owner.check()
    });
    if let Err(error) = checked {
        return window.finish_runtime_owner(owner, Err(error));
    }
    let window = match before {
        Before::Existing(existing) => {
            if let Err(error) = existing.check(&bound.runtime) {
                return window.finish_runtime_owner(owner, Err(error));
            }
            window
        }
        Before::First(creation) => {
            window.finish_runtime_owner(owner, inputs.finish(Ok(())))?;
            drop(window);
            if !creation.publish(backend, bound, owner, inputs)? {
                return Ok(None);
            }
            Window::begin()?
        }
    };
    stop::under_owner(backend, options, bound, owner, inputs, window).map(Some)
}

fn reply(stopped: Option<Value>, recovery: Value) -> Value {
    let mut value = stopped.unwrap_or_else(|| {
        json!({
            "format_version":1,"kind":"restore-runtime-lifecycle-result",
            "status":"controller_reconciled_not_started",
        })
    });
    value["operation"] = json!("recover");
    value["recovery"] = recovery;
    value
}
