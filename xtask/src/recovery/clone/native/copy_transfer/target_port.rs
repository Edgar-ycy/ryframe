//! 复制与参考恢复目标的只读窗口，沿用原初始化历史、停止证明和存储代次。
use std::{
    borrow::Borrow,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::super::{
    bridge::scope::Scope,
    cache::handoff::locks::Storage,
    export::transport::SystemTransport,
    json as document,
    resources::common,
    s3::app_configuration,
    target_verify::{
        fresh::{self, resources::Resources, selection::Selected},
        guard::{Generation, Inputs},
        input::request_configuration,
        redis,
        storage::{self, StorageInputs, SystemObserver},
        tree,
    },
};
use crate::Result;

mod history;
pub(in crate::recovery) use history::IndependentTargetBinding;
use history::{History, TargetBinding, verify_arm_storage};

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/target_port.rs"]
mod tests;

pub(in crate::recovery::clone::native) struct Target<S = Scope> {
    pub(in crate::recovery::clone::native) initialized: Value,
    pub(in crate::recovery::clone::native) request: Value,
    pub(in crate::recovery::clone::native) evidence_root: PathBuf,
    pub(in crate::recovery::clone::native) target_directory: PathBuf,
    pub(in crate::recovery::clone::native) execution: PathBuf,
    pub(in crate::recovery::clone::native) configuration: Value,
    pub(in crate::recovery::clone::native) endpoint: String,
    pub(in crate::recovery::clone::native) region: String,
    pub(in crate::recovery::clone::native) environment: BTreeMap<String, String>,
    pub(in crate::recovery::clone::native) temporary: PathBuf,
    pub(in crate::recovery::clone::native) storage: Selected,
    binding: TargetBinding,
    storage_run: PathBuf,
    observation_directory: PathBuf,
    temporary_identity: Value,
    observed_generation: Value,
    pub(super) cache: Option<Value>,
    scope: S,
    protected: Inputs,
    registered: Inputs,
}

impl Target {
    pub(in crate::recovery::clone::native) fn open(
        backend: &Path,
        manifest: &Value,
        storage_run: &Path,
        owner: Option<&Generation>,
    ) -> Result<Self> {
        if let Some(owner) = owner {
            owner.copy_session()?;
        }
        Self::bind(
            backend,
            manifest,
            storage_run,
            path(&manifest["copy_directory"], "复制目录")?,
            owner,
            || Scope::load(backend, &[], &[]),
        )
    }

    /// 首次复制在登记复制所有者前调用。续跑由各步骤核对业务前后像。
    pub(in crate::recovery::clone::native) fn verify_initial(
        &self,
        observation: &Path,
    ) -> Result<Value> {
        let TargetBinding::Copy(manifest) = &self.binding else {
            return Err("首次复制复验要求原复制清单".into());
        };
        self.check_inputs()?;
        let history = self.history(None)?;
        let active =
            history.active_storage(&self.evidence_root, &self.binding, &self.storage_run, None)?;
        let observed = fresh::verify::binding(
            &self.evidence_root,
            &json!({
                "initialized":manifest["initialized"],"observation":observation,
                "storage_run":active,"temporary_directory":self.temporary,"app_env":"test","write":true,
            }),
        )?;
        if observed["initialized"] != manifest["initialized"] {
            return Err("复制首次复验不属于原目标初始化".into());
        }
        self.check_current(None)?;
        Ok(observed)
    }

    pub(in crate::recovery::clone::native) fn check_generation(
        &self,
        owner: &Generation,
    ) -> Result<()> {
        self.check_current(Some(owner))
    }

    pub(in crate::recovery::clone::native) fn check_redis(&self, owner: &Generation) -> Result<()> {
        self.check_generation(owner)?;
        let observed = self.read_redis()?;
        if observed != self.initialized["redis"] {
            return Err("未启动目标的 Redis 当前状态与同代初始化初像不同".into());
        }
        owner.protects(&self.target_directory)?;
        self.check_inputs()
    }
}

impl<S: Borrow<Scope>> Target<S> {
    pub(super) fn bind(
        backend: &Path,
        manifest: &Value,
        storage_run: &Path,
        observation_directory: &Path,
        owner: Option<&Generation>,
        scope: impl FnOnce() -> Result<S>,
    ) -> Result<Self> {
        Self::bind_history(
            backend,
            TargetBinding::Copy(manifest.clone()),
            storage_run,
            observation_directory,
            owner,
            scope,
        )
    }

    fn bind_history(
        backend: &Path,
        binding: TargetBinding,
        storage_run: &Path,
        observation_directory: &Path,
        owner: Option<&Generation>,
        scope: impl FnOnce() -> Result<S>,
    ) -> Result<Self> {
        let history = History::read(backend, &binding, owner)?;
        let environment = common::environment(backend, binding.environment())?;
        let (temporary, temporary_identity) = temporary(backend, &environment)?;
        let storage_run = common::local_directory(backend, storage_run, "父控制器锁定的目标存储")?;
        let observation_directory =
            common::local_directory(backend, observation_directory, "目标观察目录")?;
        let inputs = StorageInputs {
            backend,
            request: &history.request,
            environment: &environment,
            observation_dir: &observation_directory,
        };
        let execution = inputs.execution_backend()?;
        let (configuration, endpoint, region) = object_binding(&inputs, &execution)?;
        let before = tree::protected_snapshot(backend, &history.target, owner)?;
        let protected = fresh::verify::inputs::protect(&inputs, &history.target, &before)?;
        let registered = Inputs::open(history.descriptors(backend, &binding)?)?;
        let scope = scope()?;
        let (observed_generation, storage, cache) = observe(
            &inputs,
            &binding,
            &history,
            scope.borrow(),
            &storage_run,
            owner,
        )?;
        history.unchanged(backend, &binding, owner)?;
        if tree::protected_snapshot(backend, &history.target, owner)? != before
            || object_binding(&inputs, &execution)?
                != (configuration.clone(), endpoint.clone(), region.clone())
            || common::environment(backend, binding.environment())? != environment
        {
            return Err("建立复制目标读窗期间输入或初始化树变化".into());
        }
        let target = Self {
            initialized: history.initialized,
            request: history.request,
            evidence_root: backend.to_owned(),
            target_directory: history.target,
            execution,
            configuration,
            endpoint,
            region,
            environment,
            temporary,
            storage,
            binding,
            storage_run,
            observation_directory,
            temporary_identity,
            observed_generation,
            cache,
            scope,
            protected,
            registered,
        };
        target.check_inputs()?;
        Ok(target)
    }

    pub(in crate::recovery::clone::native) fn storage_inputs<'a>(
        &'a self,
        observation: &'a Path,
    ) -> StorageInputs<'a> {
        StorageInputs {
            backend: &self.evidence_root,
            request: &self.request,
            environment: &self.environment,
            observation_dir: observation,
        }
    }

    pub(in crate::recovery::clone::native) fn check_inputs(&self) -> Result<()> {
        self.protected.checkpoint()?;
        self.registered.checkpoint()?;
        common::directory_identity(&self.evidence_root, &self.temporary_identity)?;
        Ok(())
    }

    fn read_redis(&self) -> Result<Value> {
        redis::state_with_observer(
            &self.storage_inputs(&self.observation_directory),
            &self.storage.value["redis"],
            true,
            true,
            &SystemObserver,
        )
    }

    fn history(&self, owner: Option<&Generation>) -> Result<History> {
        let history = History::read(&self.evidence_root, &self.binding, owner)?;
        if history.target != self.target_directory
            || history.initialized != self.initialized
            || history.request != self.request
        {
            return Err("复制目标原初始化历史或请求发生变化".into());
        }
        Ok(history)
    }

    fn check_current(&self, owner: Option<&Generation>) -> Result<()> {
        self.check_inputs()?;
        let history = self.history(owner)?;
        let (generation, selected, cache) = observe(
            &self.storage_inputs(&self.observation_directory),
            &self.binding,
            &history,
            self.scope.borrow(),
            &self.storage_run,
            owner,
        )?;
        if generation != self.observed_generation
            || selected.value != self.storage.value
            || selected.runtime != self.storage.runtime
            || cache != self.cache
        {
            return Err("复制目标停止证明、配置或已登记存储代次变化".into());
        }
        history.unchanged(&self.evidence_root, &self.binding, owner)?;
        self.protected.check()?;
        self.registered.check()?;
        self.check_inputs()
    }
}

/// 参考恢复借用阶段已持有的存储和来源窗口，只重读已完成的初始化历史。
pub(crate) struct ReferenceTarget<'a> {
    target: Target<&'a Scope>,
    storage: &'a Storage,
    observation_identity: Value,
}

impl<'a> ReferenceTarget<'a> {
    pub(crate) fn open(
        backend: &Path,
        manifest: &'a Value,
        storage: &'a Storage,
        observation: &Path,
        scope: &'a Scope,
    ) -> Result<Self> {
        let (observation, observation_identity) =
            reference_observation(backend, manifest, observation)?;
        Self::open_bound(
            backend,
            TargetBinding::Copy(manifest.clone()),
            storage,
            observation,
            observation_identity,
            scope,
        )
    }

    pub(in crate::recovery) fn open_independent(
        backend: &Path,
        binding: IndependentTargetBinding,
        storage: &'a Storage,
        observation: &Path,
        scope: &'a Scope,
    ) -> Result<Self> {
        let (observation, observation_identity) = reference_directory(backend, observation)?;
        Self::open_bound(
            backend,
            TargetBinding::Independent(Box::new(binding)),
            storage,
            observation,
            observation_identity,
            scope,
        )
    }

    fn open_bound(
        backend: &Path,
        binding: TargetBinding,
        storage: &'a Storage,
        observation: PathBuf,
        observation_identity: Value,
        scope: &'a Scope,
    ) -> Result<Self> {
        let target = reference_fence(
            || {
                storage.check()?;
                common::directory_identity(backend, &observation_identity)?;
                Ok(())
            },
            || {
                scope.assert_unchanged()?;
                let target = Target::bind_history(
                    backend,
                    binding,
                    &storage.directory,
                    &observation,
                    None,
                    || Ok(scope),
                )?;
                if target.storage_run != storage.directory
                    || target.observation_directory != observation
                {
                    return Err("参考恢复目标未绑定父存储锁或当前观察目录".into());
                }
                Ok(target)
            },
        )?;
        Ok(Self {
            target,
            storage,
            observation_identity,
        })
    }

    pub(crate) fn check(&self) -> Result<()> {
        reference_fence(|| self.check_storage(), || self.target.check_current(None))
    }

    pub(crate) fn checkpoint(&self) -> Result<()> {
        reference_fence(|| self.check_storage(), || self.target.check_inputs())
    }

    pub(crate) fn redis_state(&self) -> Result<Value> {
        reference_fence(
            || self.check_storage(),
            || {
                self.target.check_current(None)?;
                let observed = self.target.read_redis()?;
                self.target.check_inputs()?;
                Ok(observed)
            },
        )
    }

    pub(crate) fn inputs(&self) -> StorageInputs<'_> {
        self.target
            .storage_inputs(&self.target.observation_directory)
    }

    pub(crate) fn initialized(&self) -> &Value {
        &self.target.initialized
    }

    pub(crate) fn initialized_directory(&self) -> &Path {
        &self.target.target_directory
    }

    pub(crate) fn configuration(&self) -> &Value {
        &self.target.configuration
    }

    fn check_storage(&self) -> Result<()> {
        self.storage.check()?;
        if self.storage.directory != self.target.storage_run
            || common::directory_identity(&self.target.evidence_root, &self.observation_identity)?
                != self.target.observation_directory
        {
            return Err("参考恢复父存储锁或本次观察目录发生变化".into());
        }
        Ok(())
    }
}

fn reference_observation(
    backend: &Path,
    manifest: &Value,
    observation: &Path,
) -> Result<(PathBuf, Value)> {
    if manifest["copy_stage"] != "seed_to_arm" {
        return Err("参考恢复目标仅接受已验证的 seed_to_arm 清单".into());
    }
    reference_directory(backend, observation)
}

fn reference_directory(backend: &Path, observation: &Path) -> Result<(PathBuf, Value)> {
    let directory = common::local_directory(backend, observation, "参考恢复当前观察目录")?;
    let identity = directory_identity(backend, &directory)?;
    Ok((directory, identity))
}

fn reference_fence<T>(
    check: impl Fn() -> Result<()>,
    observe: impl FnOnce() -> Result<T>,
) -> Result<T> {
    check()?;
    match (observe(), check()) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(after)) => Err(format!("{error}；参考恢复目标锁后验失败：{after}").into()),
    }
}

fn observe(
    inputs: &StorageInputs<'_>,
    binding: &TargetBinding,
    history: &History,
    scope: &Scope,
    storage_run: &Path,
    owner: Option<&Generation>,
) -> Result<(Value, Selected, Option<Value>)> {
    let backend = inputs.backend;
    let active = history.active_storage(backend, binding, storage_run, owner)?;
    let resources = Resources {
        inputs: StorageInputs {
            backend,
            request: inputs.request,
            environment: inputs.environment,
            observation_dir: inputs.observation_dir,
        },
        transport: &SystemTransport,
        scope,
        run: active.as_deref(),
    };
    let (generation, selected, cache) = resources.generation_owned(owner, false)?;
    storage::generation::validate_complete(
        &history.initialized["generation"],
        &generation,
        selected.runtime.as_ref(),
        cache.as_ref(),
    )?;
    match binding {
        TargetBinding::Copy(run) if run["copy_stage"] == "seed_to_arm" => verify_arm_storage(
            backend,
            &run["target_registration"],
            &run["target_storage_run"],
            &selected,
            cache.as_ref(),
            None,
        )?,
        TargetBinding::Independent(bound) => verify_arm_storage(
            backend,
            &bound.registration,
            &bound.generation,
            &selected,
            cache.as_ref(),
            Some(bound),
        )?,
        TargetBinding::Copy(_) => {}
    }
    scope.assert_unchanged()?;
    Ok((generation, selected, cache))
}

fn object_binding(inputs: &StorageInputs<'_>, execution: &Path) -> Result<(Value, String, String)> {
    let (directory, files) =
        request_configuration::fresh_files(execution, inputs.environment, inputs.request)?;
    if inputs.request["configuration_sha256"]
        != request_configuration::configuration_identity(&directory, &files)?
    {
        return Err("复制目标 TOML 与原请求摘要不同".into());
    }
    let configuration = app_configuration::configuration_value(execution, &directory, "test")?;
    let public = app_configuration::public_configuration(execution, &configuration)?;
    let target = &inputs.request["target"]["s3"];
    if public["endpoint"] != target["endpoint"] || public["region"] != target["region"] {
        return Err("复制目标对象端点或区域与原请求不同".into());
    }
    Ok((
        configuration,
        document::text(&public["endpoint"], "目标对象端点")?.to_owned(),
        document::text(&public["region"], "目标对象区域")?.to_owned(),
    ))
}

fn temporary(backend: &Path, environment: &BTreeMap<String, String>) -> Result<(PathBuf, Value)> {
    if environment.get("APP_ENV").map(String::as_str) != Some("test") {
        return Err("复制目标必须使用已登记 APP_ENV=test".into());
    }
    let selected = |key: &str| {
        let value = environment.get(key).ok_or("目标必须登记 TEMP 和 TMP")?;
        let directory = common::local_directory(backend, Path::new(value), "目标私有临时目录")?;
        request_configuration::canonical_path(&directory)
    };
    let temporary = selected("TEMP")?;
    if selected("TMP")? != temporary {
        return Err("目标 TEMP/TMP 必须属于同一私有目录".into());
    }
    let identity = directory_identity(backend, &temporary)?;
    Ok((temporary, identity))
}

fn directory_identity(backend: &Path, directory: &Path) -> Result<Value> {
    let (device, inode) =
        common::device_inode_for(directory, &std::fs::symlink_metadata(directory)?)?;
    let identity = json!({"path":directory,"device":device,"inode":inode});
    common::directory_identity(backend, &identity)?;
    Ok(identity)
}

fn path<'a>(value: &'a Value, label: &str) -> Result<&'a Path> {
    Ok(Path::new(document::text(value, label)?))
}
