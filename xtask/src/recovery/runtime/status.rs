//! 在原恢复控制锁内观察正式运行代次，不发布生命周期或角色结果。
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{
    bind::{
        control::runtime_target,
        ownership::{Owner, control_root},
    },
    dataset_authority::{
        commands,
        inputs::Inputs,
        path,
        reads::{self, Window},
        registration::read_binding,
    },
    generation::{self, STATE},
    registration::{self, backend_root, closed_roles, protocol_path, runtime_observation},
};
use crate::{
    Result,
    cli::RuntimeControlInputs,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{digest, json as strict, target_verify::authority::require_closed},
        inputs::document,
    },
};

#[path = "status/roles.rs"]
pub(super) mod roles;

#[derive(PartialEq, Eq)]
pub(super) struct Bound {
    pub(super) runtime: PathBuf,
    pub(super) binding: Value,
    registration: Value,
    reference: PathBuf,
    target: PathBuf,
    endpoints: Value,
    scope: Value,
}

pub(super) fn run(root: &Path, options: &RuntimeControlInputs) -> Result<Value> {
    let backend = backend_root(root)?;
    let window = Window::begin()?;
    let mut inputs = Inputs::new(&backend);
    let before = Bound::read(&backend, options, &mut inputs)?;
    let control = control_root(&backend)?;
    Owner::with(&control, &before.operation("status")?, |owner| {
        let result = (|| {
            owner.check()?;
            let current = Bound::read(&backend, options, &mut inputs)?;
            if current != before {
                return Err("取得恢复运行状态锁前原登记或目标计划发生变化".into());
            }
            current.observe(&backend, &mut inputs)
        })();
        window.finish_runtime_owner(owner, inputs.finish(result))
    })
}

impl Bound {
    pub(super) fn read(
        backend: &Path,
        options: &RuntimeControlInputs,
        inputs: &mut Inputs,
    ) -> Result<Self> {
        let target_path = protocol_path(&options.target_plan)?;
        let registration_path = protocol_path(&options.runtime_registration)?;
        for file in [&target_path, &registration_path] {
            validate_local_test_path(file, backend, LocalTestPathKind::ExistingFile)?;
        }
        let target = inputs.read(&target_path)?;
        let registration = inputs.read(&registration_path)?;
        let binding = json!({
            "registration":document::binding(&registration),
            "target_plan":document::binding(&target),
        });
        let reference = read_binding(inputs, &binding, &target, &registration)?;
        let (runtime, endpoints) = runtime_target(&reference.value, &target.value)?;
        let runtime = protocol_path(&runtime)?;
        validate_local_test_path(&runtime, backend, LocalTestPathKind::StateDirectory)?;
        Ok(Self {
            runtime,
            binding,
            registration: registration.value,
            reference: reference.path,
            target: target.path,
            endpoints,
            scope: target.value["product_plan"]["scope_id"].clone(),
        })
    }

    pub(super) fn check_stop_authority(&self, authority: &Value) -> Result<()> {
        if authority["scope_id"] != self.scope
            || generation::ROLES
                .iter()
                .any(|role| authority[format!("{role}_endpoint")] != self.endpoints[*role])
        {
            return Err("恢复停止请求的 scope 或端点与原登记目标不同".into());
        }
        Ok(())
    }

    pub(super) fn operation(&self, action: &str) -> Result<String> {
        let registration =
            document::digest(&self.binding["registration"]["sha256"], "运行登记摘要")?;
        let target = document::digest(&self.binding["target_plan"]["sha256"], "目标计划摘要")?;
        Ok(format!("runtime-{action}:{registration}:{target}"))
    }

    fn observe(&self, backend: &Path, inputs: &mut Inputs) -> Result<Value> {
        let state = self.runtime.join(STATE);
        if optional_file(&state)? {
            return self.observe_generation(backend, &inputs.read(&state)?.value, inputs);
        }
        let creation = generation::creation_path(&self.runtime);
        if optional_file(&creation)? {
            return self.interrupted(backend, &creation, inputs);
        }
        self.verify_not_started(backend)?;
        Ok(registration_reply(
            "registered_not_started",
            &self.binding,
            None,
        ))
    }

    pub(super) fn verify_not_started(&self, backend: &Path) -> Result<()> {
        registration::verify_not_started(
            backend,
            &self.reference,
            &self.target,
            &path(&self.binding["registration"]["path"])?,
            &self.registration,
        )?;
        self.require_registered_directory(backend)
    }

    pub(super) fn verify_closed_roles(&self) -> Result<()> {
        closed_roles(&self.endpoints, require_closed)?;
        Ok(())
    }

    fn require_registered_directory(&self, backend: &Path) -> Result<()> {
        let (_, runtime) = runtime_observation(backend, &self.runtime)?;
        if runtime != self.registration["observation"]["runtime"] {
            return Err("目标运行目录在状态观察期间偏离登记".into());
        }
        Ok(())
    }

    fn interrupted(&self, backend: &Path, path: &Path, inputs: &mut Inputs) -> Result<Value> {
        let creation = inputs.read(path)?;
        commands::verify_direct(&creation.value["request"], inputs)?;
        let verified = generation::request(&json!({
            "action":"read-creation", "root":self.runtime, "binding":self.binding,
            "value":null, "previous":document::binding(&creation), "commands":null, "number":1,
        }))?;
        if verified != creation.value {
            return Err("首次运行 intent 在读取期间发生变化".into());
        }
        let before = runtime_observation(backend, &self.runtime)?;
        self.verify_closed_roles()?;
        if runtime_observation(backend, &self.runtime)? != before {
            return Err("首次运行目录在闭口观察期间发生变化".into());
        }
        Ok(registration_reply(
            "interrupted_before_first_generation",
            &self.binding,
            Some(1),
        ))
    }

    fn observe_generation(
        &self,
        backend: &Path,
        state: &Value,
        inputs: &mut Inputs,
    ) -> Result<Value> {
        self.validate_generation(state, inputs)?;
        let generations = strict::array(&state["generations"], "恢复运行代次集合")?;
        let current = generations.last().ok_or("恢复运行生命周期缺少代次")?;
        let observed = roles::observe(backend, current, inputs)?;
        let persisted = document::text(&current["status"], "当前代次持久状态")?;
        Ok(json!({
            "format_version":1, "kind":"restore-runtime-lifecycle-result", "operation":"status",
            "status":derived_status(persisted, &observed), "persisted_status":persisted,
            "generation":current["number"], "runtime_directory":current["directory"],
            "processes":observed, "launch":current["launch"],
        }))
    }

    pub(super) fn validate_generation(&self, state: &Value, inputs: &mut Inputs) -> Result<Value> {
        let generations = strict::array(&state["generations"], "恢复运行代次集合")?;
        let commands = generations
            .iter()
            .map(|item| commands::verify_direct(&item["request"], inputs))
            .collect::<Result<Vec<_>>>()?;
        let commands = json!(commands);
        generation::request(&json!({
            "action":"validate-state", "root":self.runtime, "binding":self.binding,
            "value":state, "previous":null, "commands":commands, "number":1,
        }))?;
        Ok(commands)
    }
}

fn registration_reply(status: &str, binding: &Value, generation: Option<u64>) -> Value {
    let mut value = json!({
        "format_version":1, "kind":"restore-runtime-lifecycle-result",
        "operation":"status", "status":status, "registration":binding,
    });
    if let Some(number) = generation {
        value["generation"] = json!(number);
    }
    value
}

pub(super) fn optional_file(path: &Path) -> Result<bool> {
    let before = receipt_present(path)?;
    if let Some(parent) = path.parent()
        && parent.try_exists()?
    {
        reads::directory(parent)?;
    }
    reads::optional(path)?;
    if receipt_present(path)? != before {
        return Err("恢复运行状态收据在观察期间出现或消失".into());
    }
    Ok(before)
}

fn receipt_present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !digest::linked(&metadata) => Ok(true),
        Ok(_) => Err("恢复运行状态收据必须是普通文件".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn derived_status<'a>(persisted: &'a str, roles: &Value) -> &'a str {
    let all = |state: &str| {
        generation::ROLES
            .iter()
            .all(|role| roles[*role]["state"] == state)
    };
    if all("running") {
        "running"
    } else if generation::ROLES
        .iter()
        .any(|role| roles[*role]["state"] == "running")
    {
        "degraded"
    } else if matches!(persisted, "stopped" | "start_failed") {
        persisted
    } else if all("stopped") {
        "exited"
    } else {
        "interrupted"
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime/status.rs"]
mod tests;
