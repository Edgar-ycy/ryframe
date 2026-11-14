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
    lifecycle::manifest,
    resources::common,
    s3::app_configuration,
    target_verify::{
        fresh::{self, resources::Resources, selection::Selected},
        guard::{Generation, Inputs},
        input::request_configuration,
        published, redis,
        storage::{self, StorageInputs, SystemObserver},
        tree,
    },
};
use crate::Result;

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
    manifest: Value,
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
        self.check_inputs()?;
        let history = self.history(None)?;
        let active =
            history.active_storage(&self.evidence_root, &self.manifest, &self.storage_run, None)?;
        let observed = fresh::verify::binding(
            &self.evidence_root,
            &json!({
                "initialized":self.manifest["initialized"],"observation":observation,
                "storage_run":active,"temporary_directory":self.temporary,"app_env":"test","write":true,
            }),
        )?;
        if observed["initialized"] != self.manifest["initialized"] {
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
        let history = History::read(backend, manifest, owner)?;
        let environment = common::environment(backend, &manifest["target_environment"])?;
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
        let registered = Inputs::open(history.descriptors(backend, manifest)?)?;
        let scope = scope()?;
        let (observed_generation, storage, cache) = observe(
            &inputs,
            manifest,
            &history,
            scope.borrow(),
            &storage_run,
            owner,
        )?;
        history.unchanged(backend, manifest, owner)?;
        if tree::protected_snapshot(backend, &history.target, owner)? != before
            || object_binding(&inputs, &execution)?
                != (configuration.clone(), endpoint.clone(), region.clone())
            || common::environment(backend, &manifest["target_environment"])? != environment
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
            manifest: manifest.clone(),
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
        let history = History::read(&self.evidence_root, &self.manifest, owner)?;
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
            &self.manifest,
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
        history.unchanged(&self.evidence_root, &self.manifest, owner)?;
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
        let target = reference_fence(
            || {
                storage.check()?;
                common::directory_identity(backend, &observation_identity)?;
                Ok(())
            },
            || {
                scope.assert_unchanged()?;
                let target = Target::bind(
                    backend,
                    manifest,
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

struct History {
    target: PathBuf,
    initialized: Value,
    request: Value,
    registration: Value,
}

impl History {
    fn read(backend: &Path, run: &Value, owner: Option<&Generation>) -> Result<Self> {
        let filename = common::bound_file(backend, &run["initialized"], "原复制目标初始化")?;
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
        if run["copy_stage"] == "seed_to_arm"
            && (run["target_registration"]
                != common::binding(&workspace.join("registration.json"))?
                || run["target_initialized_files"]
                    != common::binding(&workspace.join("initialized-files.json"))?
                || run["target_environment"] != registration["environment"])
        {
            return Err("arm 复制目标未绑定原 fresh 登记、初始化树和环境".into());
        }
        common::bound_file(backend, &run["initialized"], "原复制目标初始化")?;
        Ok(Self {
            target: target.to_owned(),
            initialized,
            request,
            registration,
        })
    }

    fn descriptors(&self, backend: &Path, run: &Value) -> Result<Vec<PathBuf>> {
        let workspace = self.target.parent().ok_or("复制目标缺少 fresh 工作区")?;
        Ok(vec![
            workspace.join("registration.json"),
            workspace.join("prepared-files.json"),
            workspace.join("initialized-files.json"),
            common::bound_file(backend, &run["target_environment"], "目标私有环境")?,
            common::bound_file(backend, &self.request["maintenance_build"], "目标维护构建")?,
        ])
    }

    fn unchanged(&self, backend: &Path, run: &Value, owner: Option<&Generation>) -> Result<()> {
        let observed = Self::read(backend, run, owner)?;
        if observed.target != self.target
            || observed.initialized != self.initialized
            || observed.request != self.request
            || observed.registration != self.registration
        {
            return Err("复制目标完整初始化历史在代次观察期间变化".into());
        }
        Ok(())
    }

    fn active_storage(
        &self,
        backend: &Path,
        run: &Value,
        storage_run: &Path,
        owner: Option<&Generation>,
    ) -> Result<Option<PathBuf>> {
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

fn observe(
    inputs: &StorageInputs<'_>,
    run: &Value,
    history: &History,
    scope: &Scope,
    storage_run: &Path,
    owner: Option<&Generation>,
) -> Result<(Value, Selected, Option<Value>)> {
    let backend = inputs.backend;
    let active = history.active_storage(backend, run, storage_run, owner)?;
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
    if run["copy_stage"] == "seed_to_arm" {
        let descriptor = &run["target_storage_run"];
        if let Some(fixture) = manifest::fixture_storage::resolve(
            backend,
            &run["target_registration"],
            Some(descriptor),
            true,
        )? {
            fixture.check(backend)?;
        } else if selected.runtime.as_ref() != Some(&descriptor["storage"])
            || cache.as_ref() != Some(&descriptor["cache"])
        {
            return Err("arm 当前 RustFS 或 Redis 代次与复制清单不同".into());
        }
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
