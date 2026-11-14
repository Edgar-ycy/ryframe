//! 在原运行登记锁内确认恢复目标仍保持三端零进程状态。
use std::{error::Error, fmt, path::Path};

use serde_json::{Value, json};

use super::{
    bind::{
        control::runtime_target,
        ownership::{Owner, control_root},
    },
    dataset_authority::{inputs::Inputs, reads::Window, registration::read_binding},
    registration,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::inputs::{
        RestoreChecks,
        document::{self, Document},
    },
};

/// 检查点只在持有同一恢复控制 owner 的正文中可用。写入期间可传入已持有的备份产物和维护来源。
pub(in crate::recovery) fn registered_stopped_runtime<T>(
    backend: &Path,
    registration_path: &Path,
    target_binding: &Value,
    body: impl FnOnce(&dyn Fn(RestoreChecks<'_>) -> Result<Value>) -> Result<T>,
) -> Result<T> {
    let backend = registration::backend_root(backend)?;
    let mut inputs = Inputs::new(&backend);
    let bound = Bound::read(&backend, registration_path, target_binding, &mut inputs)?;
    bound.check(&backend, &inputs, None, RestoreChecks::default())?;
    let control = control_root(&backend)?;
    with_checked_owner(
        &control,
        &bound.binding,
        |owner, checks| bound.check(&backend, &inputs, Some(owner), checks),
        body,
    )
}

struct Bound {
    registration: Document,
    reference: Document,
    target: Document,
    binding: Value,
}

impl Bound {
    fn read(
        backend: &Path,
        registration_path: &Path,
        target_binding: &Value,
        inputs: &mut Inputs,
    ) -> Result<Self> {
        let registration_path = registration::protocol_path(registration_path)?;
        validate_local_test_path(&registration_path, backend, LocalTestPathKind::ExistingFile)?;
        let registration = inputs.read(&registration_path)?;
        let target = inputs.bound(target_binding)?;
        let binding = json!({
            "registration":document::binding(&registration),
            "target_plan":target_binding,
        });
        let reference = read_binding(inputs, &binding, &target, &registration)?;
        Ok(Self {
            registration,
            reference,
            target,
            binding,
        })
    }

    fn check(
        &self,
        backend: &Path,
        inputs: &Inputs,
        owner: Option<&Owner>,
        checks: RestoreChecks<'_>,
    ) -> Result<()> {
        // 每次只保护本次观测的文件，恢复正文可独占发布自己的阶段证据。
        let window = Window::direct()?;
        let result = (|| {
            if let Some(owner) = owner {
                owner.check()?;
            }
            inputs.finish(Ok(()))?;
            registration::verify_not_started_with_checks(
                backend,
                &self.reference.path,
                &self.target.path,
                &self.registration.path,
                &self.registration.value,
                checks,
            )?;
            let (runtime, _) = runtime_target(&self.reference.value, &self.target.value)?;
            let (_, observed) = registration::runtime_observation(backend, &runtime)?;
            if observed != self.registration.value["observation"]["runtime"] {
                return Err("恢复运行目录在三端口核验期间偏离原登记".into());
            }
            inputs.finish(Ok(()))
        })();
        match owner {
            Some(owner) => window.finish_runtime_owner(owner, result),
            None => window.finish(result),
        }
    }
}

fn with_checked_owner<T>(
    control: &Path,
    binding: &Value,
    verify: impl Fn(&Owner, RestoreChecks<'_>) -> Result<()>,
    body: impl FnOnce(&dyn Fn(RestoreChecks<'_>) -> Result<Value>) -> Result<T>,
) -> Result<T> {
    let registration = document::digest(&binding["registration"]["sha256"], "运行登记摘要")?;
    let target = document::digest(&binding["target_plan"]["sha256"], "目标计划摘要")?;
    Owner::with(
        control,
        &format!("restore:{registration}:{target}"),
        |owner| {
            let checkpoint = |checks: RestoreChecks<'_>| {
                verify(owner, checks)?;
                Ok(binding.clone())
            };
            checkpoint(RestoreChecks::default())?;
            let result = body(&checkpoint);
            let checked = checkpoint(RestoreChecks::default());
            match (result, checked) {
                (Ok(value), Ok(_)) => Ok(value),
                (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
                (Err(cause), Err(_)) => Err(AfterFailure { cause }.into()),
            }
        },
    )
}

#[cfg(test)]
pub(crate) fn with_fixture_owner<T>(
    control: &Path,
    binding: &Value,
    verify: impl Fn() -> Result<()>,
    body: impl FnOnce(&dyn Fn() -> Result<Value>) -> Result<T>,
) -> Result<T> {
    with_checked_owner(
        control,
        binding,
        |owner, _| {
            owner.check()?;
            verify()
        },
        |checkpoint| body(&|| checkpoint(RestoreChecks::default())),
    )
}

#[derive(Debug)]
struct AfterFailure {
    cause: Box<dyn Error>,
}

impl fmt::Display for AfterFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}；恢复失败后原登记或零进程状态复核也失败，保留现场",
            self.cause
        )
    }
}

impl Error for AfterFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime/restore_guard.rs"]
mod tests;
