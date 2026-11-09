//! 状态观察只重读事实；失败不得创建锁、补写收据或重放初始化。
use super::{
    super::{binding, cli, published, tree},
    storage,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{bridge::scope::Scope, digest, json as document, resources::common},
        fixture_control::services::lifecycle::readonly::NativeRuntime,
    },
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

mod stage;

pub(crate) fn observe(backend: &Path, workspace: &Path) -> Result<Value> {
    let workspace = if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        backend.join(workspace)
    };
    let kind = if workspace.is_file() {
        LocalTestPathKind::ExistingFile
    } else {
        LocalTestPathKind::StateDirectory
    };
    validate_local_test_path(&workspace, backend, kind)?;
    let scope = Scope::load(backend, &[], &[])?;
    let result = inspect(backend, &workspace);
    // 无论状态是否有效，都不能吞掉观察期间的源码漂移。
    scope.assert_unchanged()?;
    result
}

fn project(
    backend: &Path,
    workspace: &Path,
    phase: &str,
    detail: Value,
    registration: Value,
    fixture: Value,
    controller: Value,
) -> Result<Value> {
    cli::execute(
        backend,
        workspace,
        "cli-status",
        &json!({"phase":phase,"details":detail,
        "registration":registration,"fixture":fixture,"controller":controller}),
    )
}

fn inspect(backend: &Path, workspace: &Path) -> Result<Value> {
    let report = |phase| {
        project(
            backend,
            workspace,
            phase,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        )
    };
    if !workspace.exists() {
        return report("unregistered");
    }
    let metadata = std::fs::symlink_metadata(workspace)?;
    if digest::linked(&metadata)
        || !metadata.is_dir()
        || !workspace.join("registration.json").is_file()
    {
        return report("incomplete");
    }
    let window = match Window::load(backend, workspace) {
        Ok(window) => window,
        Err(_) => return report("registration-invalid"),
    };
    let result = registered(&window);
    window.check()?;
    result
}

fn registered(window: &Window<'_>) -> Result<Value> {
    let project = |phase, detail, fixture, controller| {
        project(
            window.backend,
            window.workspace,
            phase,
            detail,
            window.binding.clone(),
            fixture,
            controller,
        )
    };
    let request = common::read_bound(window.backend, &window.value["request"], "fresh 固定请求")
        .and_then(|request| published::request_binding(window.backend, &request));
    if request.is_err() {
        return project("request-invalid", Value::Null, Value::Null, Value::Null);
    }
    let fixture = match storage::availability(
        window.backend,
        &window.storage,
        &window.value["storage_run"],
    ) {
        Ok(value) => value,
        Err(_) => return project("fixture-invalid", Value::Null, Value::Null, Value::Null),
    };
    let before = match current_tree(window) {
        Ok(files) => files,
        Err(_) => return project("tree-invalid", Value::Null, Value::Null, Value::Null),
    };
    let (phase, detail) = stage::observe(window).unwrap_or(("local-invalid", Value::Null));
    // 已集成原生端口同时复核完整 state binding 和实际进程身份。
    let controller = match NativeRuntime::new(window.backend).observe_controller(&window.storage) {
        Ok(Value::Null) => Value::Null,
        Ok(mut value) => {
            value["run_directory"] = json!(window.storage);
            value
        }
        Err(_) => json!("invalid"),
    };
    let result = project(phase, detail, fixture, controller)?;
    if current_tree(window)? != before {
        return Err("fresh 目标文件在 status 只读观察期间变化".into());
    }
    Ok(result)
}

fn current_tree(window: &Window<'_>) -> Result<Option<Value>> {
    if window.target.exists() {
        tree::snapshot(window.backend, &window.target, false).map(Some)
    } else {
        Ok(None)
    }
}

struct Window<'a> {
    backend: &'a Path,
    workspace: &'a Path,
    target: PathBuf,
    storage: PathBuf,
    value: Value,
    binding: Value,
    environment: Value,
    storage_state: Value,
}

impl<'a> Window<'a> {
    fn load(backend: &'a Path, workspace: &'a Path) -> Result<Self> {
        let observed = cli::execute(backend, workspace, "cli-registration", &json!({}))?;
        let value = observed["value"].clone();
        let environment =
            binding::private_environment(backend, &value["environment"])?["environment"].clone();
        let storage = storage::load(backend, &value["storage_run"])?;
        let window = Self {
            backend,
            workspace,
            target: PathBuf::from(document::text(
                &value["target_directory"],
                "fresh 目标目录",
            )?),
            binding: observed["binding"].clone(),
            environment,
            storage_state: common::binding(&storage.join("state.json"))?,
            storage,
            value,
        };
        window.check()?;
        Ok(window)
    }

    fn check(&self) -> Result<()> {
        let filename = self.workspace.join("registration.json");
        if common::binding(&filename)? != self.binding
            || document::read(&filename)? != self.value
            || binding::private_environment(self.backend, &self.value["environment"])?["environment"]
                != self.environment
            || storage::load(self.backend, &self.value["storage_run"])? != self.storage
            || common::binding(&self.storage.join("state.json"))? != self.storage_state
        {
            return Err("fresh 登记、私有环境或完整存储历史在只读观察期间变化".into());
        }
        common::bound_file(self.backend, &self.value["request"], "fresh 固定请求")?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/target_verify/fresh/status.rs"]
mod tests;
