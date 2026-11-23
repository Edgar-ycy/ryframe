use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::super::super::{
    cache::handoff::locks::registered_target,
    json as document,
    lifecycle::manifest,
    resources::common,
    target_verify::{
        fresh::{self, selection::Selected},
        guard::Generation,
        published,
    },
};
use crate::Result;

/// 已核独立 ARM 提供的原目标描述符，由外层在本次参考恢复窗口内消费。
#[derive(Clone)]
pub(in crate::recovery) struct IndependentTargetBinding {
    pub(in crate::recovery) initialized: Value,
    pub(in crate::recovery) registration: Value,
    pub(in crate::recovery) initialized_files: Value,
    pub(in crate::recovery) environment: Value,
    pub(in crate::recovery) generation: Value,
    pub(in crate::recovery) state: Value,
}

pub(super) enum TargetBinding {
    Copy(Value),
    Independent(Box<IndependentTargetBinding>),
}

impl TargetBinding {
    fn initialized(&self) -> &Value {
        match self {
            Self::Copy(run) => &run["initialized"],
            Self::Independent(bound) => &bound.initialized,
        }
    }

    pub(super) fn environment(&self) -> &Value {
        match self {
            Self::Copy(run) => &run["target_environment"],
            Self::Independent(bound) => &bound.environment,
        }
    }

    fn original_history(&self) -> Option<(&Value, &Value)> {
        match self {
            Self::Independent(bound) => Some((&bound.registration, &bound.initialized_files)),
            Self::Copy(run) if run["copy_stage"] == "seed_to_arm" => Some((
                &run["target_registration"],
                &run["target_initialized_files"],
            )),
            Self::Copy(_) => None,
        }
    }
}

pub(super) struct History {
    pub(super) target: PathBuf,
    pub(super) initialized: Value,
    pub(super) request: Value,
    registration: Value,
}

impl History {
    pub(super) fn read(
        backend: &Path,
        binding: &TargetBinding,
        owner: Option<&Generation>,
    ) -> Result<Self> {
        let initialized_binding = binding.initialized();
        let filename = common::bound_file(backend, initialized_binding, "原复制目标初始化")?;
        let target = filename.parent().ok_or("复制目标缺少初始化目录")?;
        let workspace = target.parent().ok_or("复制目标缺少 fresh 工作区")?;
        if filename != target.join("initialized.json") || target != workspace.join("target") {
            return Err("复制目标初始化不属于固定 fresh 工作区".into());
        }
        let (initialized, request) = if let Some(owner) = owner {
            published::owned_history(backend, &filename, owner)?
        } else {
            published::unlocked_history(backend, &filename)?
        };
        let registration = document::read(&workspace.join("registration.json"))?;
        let prepared = document::read(&target.join("prepare.json"))?;
        if registration["format_version"] != 1
            || registration["kind"] != "devex-clone-fresh-target-registration"
            || registration["target_directory"] != json!(target)
            || prepared["request"] != registration["request"]
        {
            return Err("复制目标初始化与原 fresh 登记或请求不同".into());
        }
        if let Some((registered, files)) = binding.original_history()
            && (*registered != common::binding(&workspace.join("registration.json"))?
                || *files != common::binding(&workspace.join("initialized-files.json"))?
                || *binding.environment() != registration["environment"])
        {
            return Err("arm 复制目标未绑定原 fresh 登记、初始化树和环境".into());
        }
        common::bound_file(backend, initialized_binding, "原复制目标初始化")?;
        Ok(Self {
            target: target.to_owned(),
            initialized,
            request,
            registration,
        })
    }

    pub(super) fn descriptors(
        &self,
        backend: &Path,
        binding: &TargetBinding,
    ) -> Result<Vec<PathBuf>> {
        let workspace = self.target.parent().ok_or("复制目标缺少 fresh 工作区")?;
        Ok(vec![
            workspace.join("registration.json"),
            workspace.join("prepared-files.json"),
            workspace.join("initialized-files.json"),
            common::bound_file(backend, binding.environment(), "目标私有环境")?,
            common::bound_file(backend, &self.request["maintenance_build"], "目标维护构建")?,
        ])
    }

    pub(super) fn unchanged(
        &self,
        backend: &Path,
        binding: &TargetBinding,
        owner: Option<&Generation>,
    ) -> Result<()> {
        let observed = Self::read(backend, binding, owner)?;
        if observed.target != self.target
            || observed.initialized != self.initialized
            || observed.request != self.request
            || observed.registration != self.registration
        {
            return Err("复制目标完整初始化历史在代次观察期间变化".into());
        }
        Ok(())
    }

    pub(super) fn active_storage(
        &self,
        backend: &Path,
        binding: &TargetBinding,
        storage_run: &Path,
        owner: Option<&Generation>,
    ) -> Result<Option<PathBuf>> {
        let run = match binding {
            TargetBinding::Copy(run) => run,
            TargetBinding::Independent(bound) => {
                return bound.locked_storage(backend, storage_run).map(Some);
            }
        };
        match run["copy_stage"].as_str() {
            Some("seed_to_arm") => {
                let registered = manifest::target_storage_path(backend, run)?
                    .ok_or("arm 复制缺少已登记目标存储")?;
                if registered != storage_run
                    || fresh::storage_run(backend, &self.registration["storage_run"], owner)?
                        != registered
                {
                    return Err("arm 目标存储不是父控制器锁定的登记目录".into());
                }
                Ok(Some(registered))
            }
            Some("source_to_seed") => {
                let descriptor = &self.registration["storage_run"];
                if descriptor.is_null()
                    || descriptor.as_object().is_some_and(|value| value.is_empty())
                {
                    return Ok(None);
                }
                let registered = fresh::storage_run(backend, descriptor, owner)?;
                if document::read(&registered.join("manifest.json"))?["kind"]
                    != "reference-fixture-service-run"
                {
                    return Ok(None);
                }
                if registered != storage_run {
                    return Err("seed 夹具存储不是父控制器锁定的登记目录".into());
                }
                Ok(Some(registered))
            }
            _ => Err("目标读窗不支持未登记的复制阶段".into()),
        }
    }
}

impl IndependentTargetBinding {
    pub(super) fn locked_storage(&self, backend: &Path, storage_run: &Path) -> Result<PathBuf> {
        let registered = registered_target(backend, &self.registration, &self.generation)?;
        if registered != storage_run {
            return Err("独立 B1 目标存储不是父控制器锁定的登记目录".into());
        }
        let state = common::bound_file(backend, &self.state, "独立 ARM 目标存储账本")?;
        if state != registered.join("state.json") {
            return Err("独立 ARM 目标存储账本不属于父控制器锁定的登记目录".into());
        }
        Ok(registered)
    }
}

pub(super) fn verify_arm_storage(
    backend: &Path,
    registration: &Value,
    descriptor: &Value,
    selected: &Selected,
    cache: Option<&Value>,
    independent: Option<&IndependentTargetBinding>,
) -> Result<()> {
    let runtime = selected.runtime.as_ref();
    if let Some(fixture) =
        manifest::fixture_storage::resolve(backend, registration, Some(descriptor), true)?
    {
        if let Some(bound) = independent {
            if fixture.state_binding() != &bound.state {
                return Err("独立 ARM 夹具账本与登记时不同".into());
            }
            verify_fixture_generation(&fixture.binding["generation"], runtime, cache)?;
        }
        fixture.check(backend)?;
    } else {
        if independent.is_some() {
            document::exact(
                descriptor,
                &["path", "manifest", "storage", "cache"],
                "独立 ARM 标准存储代次",
            )?;
        }
        if runtime != Some(&descriptor["storage"]) || cache != Some(&descriptor["cache"]) {
            let reason = if independent.is_some() {
                "独立 ARM 当前 RustFS 或 Redis 代次与原登记不同"
            } else {
                "arm 当前 RustFS 或 Redis 代次与复制清单不同"
            };
            return Err(reason.into());
        }
    }
    Ok(())
}

pub(super) fn verify_fixture_generation(
    expected: &Value,
    runtime: Option<&Value>,
    cache: Option<&Value>,
) -> Result<()> {
    let matches = if expected["kind"] == "initial" {
        runtime.is_none() && cache.is_none()
    } else {
        runtime.and_then(|value| value.get("generation")) == Some(expected)
            && cache.and_then(|value| value.get("generation")) == Some(expected)
    };
    if !matches {
        return Err("独立 ARM 当前夹具 RustFS 或 Redis 代次与原登记不同".into());
    }
    Ok(())
}
