//! 固定 post-copy/API 控制只消费已发布交接；停止和锁恢复不依赖对侧来源文件。
use super::{
    diagnostic::{self, Phase},
    locks, registration,
};
use crate::{
    Result,
    cli::{CloneRole, CloneRuntimeOperation, CloneRuntimeOptions, CloneSide},
    recovery::{
        clone::native::{
            digest, export, json as document,
            lifecycle::{
                identity,
                stage::{StageRequest, StageRunner},
                state,
            },
            post_process::api_successor,
            resources::common,
            seed,
            target_verify::guard::Inputs,
        },
        full_stack::worker::{controller, role::Role},
    },
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[path = "observation/recovery.rs"]
mod recovery;
#[path = "observation/retirement.rs"]
pub(super) mod retirement;

pub(super) struct Runner<'a> {
    pub options: &'a CloneRuntimeOptions,
}

impl StageRunner for Runner<'_> {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if request.stage != "runtime-target"
            || request.mode != self.options.operation.as_str()
            || self.options.side != CloneSide::Target
            || matches!(
                self.options.operation,
                CloneRuntimeOperation::Start
                    | CloneRuntimeOperation::RestartApi
                    | CloneRuntimeOperation::ReconcileApi
            )
            || request.run_dir != self.options.run_dir
        {
            return Err("已监督API控制必须绑定当前固定target操作".into());
        }
        if self.options.roles != [CloneRole::Api] {
            return Err("复制后交接只允许API，Worker必须保持停止".into());
        }
        if self.options.operation == CloneRuntimeOperation::Status {
            let post = seed::history::post(request.backend, request.run_dir, None, None, true)?;
            if api_successor::load(request.backend, request.run_dir, &post.descriptor)?.is_some() {
                return Ok(());
            }
        }
        retirement::preflight(request.backend, request.run_dir)
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        let phase = match self.options.operation {
            CloneRuntimeOperation::Status => Phase::Observe,
            CloneRuntimeOperation::Stop => Phase::BeforeStop,
            CloneRuntimeOperation::Recover => Phase::Recover,
            CloneRuntimeOperation::Start
            | CloneRuntimeOperation::RestartApi
            | CloneRuntimeOperation::ReconcileApi => {
                return Err("启动与接续核对必须使用相应原生入口".into());
            }
        };
        diagnostic::context(
            request.run_dir,
            request.attempt,
            phase,
            (|| {
                self.preflight(request)?;
                let frame = Frame::load(request, self.options.operation)?;
                match self.options.operation {
                    CloneRuntimeOperation::Status => frame.status(),
                    CloneRuntimeOperation::Stop => stop(&frame),
                    CloneRuntimeOperation::Recover => recovery::run(&frame),
                    CloneRuntimeOperation::Start
                    | CloneRuntimeOperation::RestartApi
                    | CloneRuntimeOperation::ReconcileApi => unreachable!(),
                }
            })(),
        )
    }
}

pub(super) struct Frame {
    pub backend: PathBuf,
    pub product: PathBuf,
    pub run: PathBuf,
    pub number: u64,
    pub post: seed::history::Post,
    pub directory: PathBuf,
    pub runtime: Value,
    successor: Option<Value>,
    environment: Value,
    state: Value,
    cleanup: bool,
    retirement: Option<retirement::Retirement>,
    protected: Inputs,
    roles: Vec<(PathBuf, Option<Value>)>,
}

impl Frame {
    fn load(request: &StageRequest<'_>, operation: CloneRuntimeOperation) -> Result<Self> {
        let cleanup = operation != CloneRuntimeOperation::Status;
        let saved = state::load(request.run_dir, !cleanup)?;
        let number = request.attempt.ok_or("API控制缺少当前阶段序号")?;
        let last = state::attempts(&saved)?.last().ok_or("API控制阶段缺失")?;
        let (_, owner) = state::controller_record(request.run_dir, number, last)?;
        if last["number"].as_u64() != Some(number)
            || last["stage"] != "runtime-target"
            || last["mode"] != operation.as_str()
            || last["status"] != "running"
            || owner != document::read(&request.run_dir.join("run.lock/owner.json"))?
            || owner["identity"] != identity::current()?
        {
            return Err("API控制不属于当前真实阶段控制器和运行锁".into());
        }
        let post = seed::history::post(request.backend, request.run_dir, None, None, true)?;
        common::local_directory(request.backend, &post.runtime, "已发布API运行目录")?;
        let environment = post.request["api_environment"].clone();
        let successor = api_successor::load(request.backend, request.run_dir, &post.descriptor)?;
        let mut files = stage_files(request, &post, number, &environment)?;
        let directory = active_directory(request.backend, &post, successor.as_ref(), &mut files)?;
        let roles = role_files(request.backend, &directory)?;
        files.extend(
            roles
                .iter()
                .filter(|(_, binding)| binding.is_some())
                .map(|(path, _)| path.clone()),
        );
        let retirement = if cleanup || successor.is_none() {
            retirement::Retirement::load(request.backend, request.run_dir, &post, &saved)?
        } else {
            None
        };
        if let Some(retirement) = &retirement {
            files.extend(retirement.paths(request.backend)?);
        }
        let protected = Inputs::open(files)?;
        let runtime = registration::cleanup_runtime(request.backend, request.run_dir, &post)?;
        let product = crate::recovery::clone::native::post_process::runtime::product_root(
            request.backend,
            &runtime,
        )?;
        let frame = Self {
            backend: request.backend.to_owned(),
            product,
            run: request.run_dir.to_owned(),
            number,
            post,
            directory,
            runtime,
            successor,
            environment,
            state: saved,
            cleanup,
            retirement,
            protected,
            roles,
        };
        frame.check()?;
        Ok(frame)
    }

    pub fn check(&self) -> Result<()> {
        self.protected.check()?;
        if let Some(retirement) = &self.retirement {
            retirement.check(&self.backend)?;
        } else if self.successor.is_none() {
            super::guard::absent(&self.run.join("seed-runtime.json"))?;
        }
        if role_files(&self.backend, &self.directory)? != self.roles {
            return Err("API或Worker角色收据、完整树或原缺失事实在观察期间变化".into());
        }
        if state::load(&self.run, !self.cleanup)? != self.state {
            return Err("API控制完整阶段账本在操作期间变化".into());
        }
        let post = seed::history::post(&self.backend, &self.run, None, None, true)?;
        if post.request != self.post.request
            || post.descriptor != self.post.descriptor
            || post.runtime != self.post.runtime
            || registration::cleanup_runtime(&self.backend, &self.run, &post)? != self.runtime
            || api_successor::load(&self.backend, &self.run, &post.descriptor)? != self.successor
        {
            return Err("API控制活动登记或准备合同在操作期间变化".into());
        }
        let handoff = document::read(&self.post.runtime.join("handoff.json"))?;
        let directory = Path::new(document::text(
            &self.runtime["api_config_dir"],
            "API配置目录",
        )?);
        let table = serde_json::to_value(
            crate::rate_limit::load_app_table(&self.product, Some(directory), "test")
                .map_err(|_| "API控制无法读取原已绑定文件配置")?,
        )?;
        export::proof::verify_api_address(document::text(&handoff["api_url"], "API地址")?, &table)?;
        self.protected.check()
    }

    pub fn observed(&self) -> Result<Value> {
        self.worker_stopped()?;
        let api = controller::observe_bound(
            Role::Api,
            &self.product,
            &self.directory,
            &self.environment,
        )?;
        if api["state"] == "running" {
            let actual =
                crate::recovery::clone::native::target_verify::storage::protocol::arguments(
                    &self.backend,
                    &self.directory,
                    &common::binding(&self.directory.join("api.json"))?,
                    &api["identity"],
                )?;
            if actual["actual_arguments_sha256"]
                != digest::plan_hash(&json!([
                    self.runtime["artifacts"]["api"]["path"],
                    "--config-dir",
                    self.runtime["api_config_dir"]
                ]))?
            {
                return Err("API完整实际参数不同于冻结产物和明确配置目录".into());
            }
        }
        self.check()?;
        Ok(api)
    }

    fn worker_stopped(&self) -> Result<()> {
        self.check()?;
        let worker = match legacy_lock_only(&self.backend, &self.directory)? {
            Some(owner) => controller::observe_worker_under_outer_lock(
                &self.product,
                &self.directory,
                &self.environment,
                &owner,
            )?,
            None => controller::observe_bound(
                Role::Worker,
                &self.product,
                &self.directory,
                &self.environment,
            )?,
        };
        if worker["state"] != "stopped" {
            return Err("复制后Worker完整树仍运行，不能控制固定API交接".into());
        }
        self.check()
    }

    fn history_identity(&self, history: &super::history::Closing) -> Result<()> {
        if self.roles.iter().any(|(path, binding)| {
            path.file_name().is_some_and(|name| name == "api.json") && binding.is_some()
        }) {
            if history.binding.is_none() {
                return Err("API生产者历史未形成完整已启动代次，保留未知现场".into());
            }
            let receipt = document::read(&self.directory.join("api.json"))?;
            if history
                .identity
                .as_ref()
                .is_some_and(|identity| *identity != receipt["identity"])
            {
                return Err("API生产者历史未形成完整已启动代次，保留未知现场".into());
            }
        }
        Ok(())
    }

    fn status(&self) -> Result<Value> {
        let observed = self.observed()?;
        Ok(
            json!({"format_version":1,"kind":"devex-clone-runtime-observation","operation":"status",
            "scope_id":self.runtime["scope_id"],"runtime_directory":self.directory,
            "processes":{"api":process(&observed)}}),
        )
    }
}

fn stage_files(
    request: &StageRequest<'_>,
    post: &seed::history::Post,
    number: u64,
    environment: &Value,
) -> Result<Vec<PathBuf>> {
    let mut files = vec![
        request.run_dir.join("manifest.json"),
        request.run_dir.join("state.json"),
        request.run_dir.join("run.lock/owner.json"),
        state::controller_path(request.run_dir, number),
        common::bound_file(request.backend, &post.descriptor, "活动API登记")?,
        common::bound_file(request.backend, environment, "冻结API环境")?,
    ];
    for name in ["runtime.json", "binaries.json", "handoff.json"] {
        files.push(common::common_file(
            request.backend,
            &post.runtime.join(name),
            "API交接固定文件",
        )?);
    }
    Ok(files)
}

fn active_directory(
    backend: &Path,
    post: &seed::history::Post,
    successor: Option<&Value>,
    files: &mut Vec<PathBuf>,
) -> Result<PathBuf> {
    let Some(successor) = successor else {
        return Ok(post.runtime.clone());
    };
    let directory = common::local_directory(
        backend,
        Path::new(document::text(
            &successor["runtime_directory"],
            "API后继目录",
        )?),
        "API后继运行目录",
    )?;
    if directory == post.runtime {
        return Err("API后继不能使用原历史运行目录".into());
    }
    files.push(common::bound_file(
        backend,
        &successor["intent"],
        "API后继意图",
    )?);
    for name in ["runtime.json", "binaries.json"] {
        let file = common::common_file(backend, &directory.join(name), "API后继固定文件")?;
        if digest::file(&file)? != digest::file(&post.runtime.join(name))? {
            return Err("API后继运行合同或构建清单与原冻结字节不同".into());
        }
        files.push(file);
    }
    Ok(directory)
}

fn role_files(backend: &Path, directory: &Path) -> Result<Vec<(PathBuf, Option<Value>)>> {
    [
        "api.json",
        "api-tree.json",
        "worker.json",
        "worker-tree.json",
    ]
    .into_iter()
    .map(|name| {
        let path = directory.join(name);
        let binding = match std::fs::symlink_metadata(&path) {
            Ok(_) => Some(common::binding(&common::common_file(
                backend,
                &path,
                "固定角色证据",
            )?)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok((path, binding))
    })
    .collect()
}

fn legacy_lock_only(backend: &Path, directory: &Path) -> Result<Option<Value>> {
    let path = directory.join("worker-control.lock");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    common::local_directory(backend, &path, "原外层控制锁")?;
    let owner = common::read_bound(
        backend,
        &common::binding(&path.join("owner.json"))?,
        "原外层owner",
    )?;
    if owner["kind"] != "devex-clone-runtime-lock" {
        return Err("Worker或未知控制器尚有未完成操作，不能推定Worker完整树已停止".into());
    }
    recovery::same_runtime(backend, &owner["runtime_directory"], directory)?;
    Ok(Some(owner))
}

fn stop(frame: &Frame) -> Result<Value> {
    let lock = locks::Runtime::acquire_for(&frame.backend, &frame.directory, "stop")?;
    let result = diagnostic::context(
        &frame.run,
        Some(frame.number),
        Phase::BeforeStop,
        (|| {
            // 停止资格由原生controller在同一角色guard中复核，不能先要求待停止API仍ready。
            frame.worker_stopped()?;
            // 历史不是停止权限；失败保留至完整树关闭后报告，不能阻止显式回收原登记API。
            let history = super::history::closing(&frame.backend, &frame.product, &frame.directory)
                .and_then(|history| {
                    frame.history_identity(&history)?;
                    Ok(history)
                });
            let history = if frame.retirement.is_some() {
                Ok(history?)
            } else {
                history
            };
            lock.check(&frame.backend)?;
            let result = match controller::control_bound(
                Role::Api,
                "stop",
                &frame.product,
                &frame.directory,
                60.0,
                &frame.environment,
            ) {
                Ok(result) => result,
                Err(cause) => return Err(diagnostic::failure(Phase::Stop, cause, history.err())),
            };
            diagnostic::context(
                &frame.run,
                Some(frame.number),
                Phase::AfterStop,
                (|| {
                    let after = frame.observed()?;
                    if result["state"] != "stopped" || after["state"] != "stopped" {
                        return Err(
                            "API停止没有取得完整成员关闭证明，保留原操作，不追加停止".into()
                        );
                    }
                    lock.check(&frame.backend)?;
                    let history = history?;
                    super::history::closed(
                        &frame.backend,
                        &frame.directory,
                        &frame.runtime,
                        &history,
                    )?;
                    frame.check()?;
                    Ok(stopped_result(frame, &after, history.binding.is_some()))
                })(),
            )
        })(),
    );
    diagnostic::context(
        &frame.run,
        Some(frame.number),
        Phase::AfterStop,
        lock.finish(&frame.backend, result),
    )
}

fn stopped_result(frame: &Frame, observed: &Value, has_history: bool) -> Value {
    let history = has_history.then(|| frame.directory.join("producer-history.json"));
    let mut result = json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":"stop",
        "scope_id":frame.runtime["scope_id"],"runtime_directory":frame.directory,
        "processes":{"api":process(observed)},"producer_history":history});
    if let Some(retirement) = &frame.retirement {
        let mut evidence = retirement.evidence();
        if let Some(successor) = &frame.successor {
            evidence["successor"] = json!({"intent":successor["intent"],
                "api_process":successor["api"],"api_tree":successor["api_tree"]});
        }
        result["seed_retirement"] = evidence;
    }
    result
}

fn process(observed: &Value) -> Value {
    json!({"state":observed["state"],"identity":observed["identity"],"ready":observed["state"]=="running"})
}
