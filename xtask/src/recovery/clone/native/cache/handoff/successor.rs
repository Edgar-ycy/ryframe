//! 保存原事故与标记恢复收据，按一次性预留启动新 API，并单独归档实际启动结果。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{
    diagnostic::{self, Phase},
    guard,
    history::Producer,
    locks::Storage,
    registration, selected,
};
use crate::{
    Result,
    cli::{CloneRole, CloneRuntimeOperation, CloneRuntimeOptions},
    recovery::{
        clone::native::{
            bridge::scope::Scope,
            copy_transfer::target_port::Target,
            json as document,
            lifecycle::{
                identity,
                lock::ProcessGuard,
                manifest,
                stage::{self, StageRequest, StageRunner},
                state,
            },
            post_process::{
                api_successor::prepare_frozen_runtime,
                runtime::{listener, product_root},
            },
            resources::common,
            seed::history,
            target_verify::{
                guard::{Generation, Inputs},
                redis,
                storage::SystemObserver,
            },
        },
        full_stack::worker::{controller, role::Role, start},
    },
};

mod evidence;
mod second;
use evidence::{Intent, Markers};
use second::Second;

pub(crate) fn run(backend: &Path, options: &CloneRuntimeOptions) -> Result<Value> {
    if !selected(backend, options)? || options.roles != [CloneRole::Api] || !cfg!(windows) {
        return Err("API 事故接续仅支持 Windows 已登记目标 API".into());
    }
    let reconcile = match options.operation {
        CloneRuntimeOperation::RestartApi => false,
        CloneRuntimeOperation::ReconcileApi => true,
        _ => return Err("API 后继只接受定向启动或原操作核对".into()),
    };
    let requested = options
        .request
        .as_deref()
        .map(|path| Second::bind_request(backend, path))
        .transpose()?;
    stage::run_named(
        backend,
        &options.run_dir,
        "runtime-target",
        options.operation.as_str(),
        &Runner {
            reconcile,
            requested,
        },
    )
}

struct Runner {
    reconcile: bool,
    requested: Option<Value>,
}

impl StageRunner for Runner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        let saved = state::load(request.run_dir, true)?;
        if let Some(requested) = &self.requested {
            let post = history::post(request.backend, request.run_dir, None, None, false)?;
            let second = Second::load(request, &saved, &post, requested)?;
            let marker = Markers::before(request, &saved, second.original_restart)?;
            second.check_marker(&marker)?;
            second.paths(request.backend)?;
            if let Some(number) = second.pending {
                Intent::load(request, &saved, Some((&second, number)))?;
            }
            return Ok(());
        }
        Markers::load(request, &saved)?;
        let root = evidence::directory(request.run_dir);
        if self.reconcile {
            Intent::load(request, &saved, None)?;
        } else {
            guard::absent(&root.join("intent.json"))?;
            guard::absent(&root.join("published.json"))?;
        }
        Ok(())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        let frame = diagnostic::context(
            request.run_dir,
            request.attempt,
            Phase::Binding,
            Frame::load(request, self.requested.as_ref()),
        )?;
        let storage = Storage::acquire(request.backend, request.run_dir, &frame.manifest)?;
        let result = self.with_storage(&frame, &storage);
        storage.finish(result)
    }
}

impl Runner {
    fn with_storage(&self, frame: &Frame<'_>, storage: &Storage) -> Result<Value> {
        let backend = frame.request.backend;
        let target_file = common::bound_file(
            backend,
            &frame.manifest["initialized"],
            "后继 API 原目标初始化",
        )?;
        let target_dir = target_file.parent().ok_or("缺少原目标初始化目录")?;
        let copy = Path::new(document::text(
            &frame.manifest["copy_directory"],
            "原复制目录",
        )?);
        let ledger = common::local_directory(backend, &copy.join("ledger"), "原复制账本")?;
        let owner = Generation::acquire(backend, target_dir)?;
        let result = self.with_process_guards(frame, storage, &owner, copy, &ledger);
        owner.finish(result)
    }

    fn with_process_guards(
        &self,
        frame: &Frame<'_>,
        storage: &Storage,
        owner: &Generation,
        copy: &Path,
        ledger: &Path,
    ) -> Result<Value> {
        let original = &frame.post.runtime;
        let copy_guard = ProcessGuard::acquire_existing_named(ledger, "copy-ledger.guard")?;
        let original_guard =
            ProcessGuard::acquire_existing_named(original, "runtime-control.guard")?;
        let api_guard = ProcessGuard::acquire_existing_named(original, "api-control.guard")?;
        let prior_api_guard = frame
            .second
            .as_ref()
            .map(|second| {
                ProcessGuard::acquire_existing_named(&second.runtime_directory, "api-control.guard")
            })
            .transpose()?;
        guard::absent(&ledger.join("clone.lock"))?;
        let check_context = || {
            storage.check()?;
            copy_guard.check()?;
            original_guard.check()?;
            api_guard.check()?;
            if let Some(guard) = &prior_api_guard {
                guard.check()?;
            }
            guard::absent(&ledger.join("clone.lock"))?;
            frame.check()
        };
        self.execute_in_target(frame, storage, owner, copy, &check_context)
    }

    fn execute_in_target(
        &self,
        frame: &Frame<'_>,
        storage: &Storage,
        owner: &Generation,
        copy: &Path,
        check_context: &dyn Fn() -> Result<()>,
    ) -> Result<Value> {
        let backend = frame.request.backend;
        check_context()?;
        let pending = if self.reconcile {
            let second = frame
                .second
                .as_ref()
                .map(|plan| {
                    plan.pending
                        .map(|number| (plan, number))
                        .ok_or("第二代 API 核对缺少原启动阶段")
                })
                .transpose()?;
            let intent = Intent::load(frame.request, &frame.saved, second)?;
            let identity = frame.live_identity(&intent)?;
            Some((intent, identity))
        } else {
            None
        };
        let scope = || Scope::load(backend, &[], &[]);
        let target = if pending.is_some() {
            Target::bind_for_api_successor(
                backend,
                &frame.manifest,
                &storage.directory,
                copy,
                Some(owner),
                &frame.api_url,
                scope,
            )?
        } else {
            frame.diagnose(
                Phase::BeforeStart,
                Target::bind(
                    backend,
                    &frame.manifest,
                    &storage.directory,
                    copy,
                    Some(owner),
                    scope,
                ),
            )?
        };
        if target.storage.value["redis"] != frame.marker.redis {
            return Err("API 后继选择的 Redis 代次不同于标记确认".into());
        }
        let check_closed = || {
            check_context()?;
            target.check_generation(owner)
        };
        let check_live = |intent: &Intent, identity: &Value| {
            check_context()?;
            frame.check_active(intent, identity, &target, owner, copy)?;
            check_context()
        };
        if let Some((intent, identity)) = pending {
            return frame.reconcile(&intent, &identity, &check_live);
        }
        frame.diagnose(
            Phase::BeforeStart,
            (|| {
                check_closed()?;
                if frame.second.is_some() {
                    // 第一代 API 已使用同代 Redis，原会话由后续独立和解处理。
                    Frame::verify_markers(&target, copy)?;
                    check_closed()
                } else {
                    target.check_redis(owner)
                }
            })(),
        )?;
        frame.start(&check_closed, &check_live)
    }
}

struct Frame<'a> {
    request: &'a StageRequest<'a>,
    manifest: Value,
    saved: Value,
    marker: Markers,
    post: history::Post,
    runtime: Value,
    product: PathBuf,
    api_url: String,
    controller: Value,
    protected: Inputs,
    second: Option<Second>,
}

impl<'a> Frame<'a> {
    fn diagnose<T>(&self, phase: Phase, result: Result<T>) -> Result<T> {
        diagnostic::context(self.request.run_dir, self.request.attempt, phase, result)
    }

    fn load(request: &'a StageRequest<'a>, requested: Option<&Value>) -> Result<Self> {
        let (backend, run) = (request.backend, request.run_dir);
        let (saved, controller) = evidence::current(request)?;
        let manifest = manifest::load(backend, &run.join("manifest.json"))?.value;
        let post = history::post(backend, run, None, None, false)?;
        let second = requested
            .map(|binding| Second::load(request, &saved, &post, binding))
            .transpose()?;
        let marker = if let Some(plan) = &second {
            let marker = Markers::before(request, &saved, plan.original_restart)?;
            plan.check_marker(&marker)?;
            marker
        } else {
            Markers::load(request, &saved)?
        };
        let original = post.runtime.clone();
        let mut paths = vec![
            run.join("manifest.json"),
            run.join("state.json"),
            run.join("run.lock/owner.json"),
            state::controller_path(run, request.attempt.ok_or("后继阶段序号缺失")?),
        ];
        paths.push(common::bound_file(
            backend,
            &post.descriptor,
            "原 API 登记",
        )?);
        paths.push(common::bound_file(
            backend,
            &post.request["api_environment"],
            "原 API 冻结环境",
        )?);
        paths.extend(
            [
                "runtime.json",
                "binaries.json",
                "handoff.json",
                "producer-history.json",
            ]
            .map(|name| original.join(name)),
        );
        paths.extend(marker.paths(backend)?);
        if let Some(plan) = &second {
            paths.extend(plan.paths(backend)?);
        }
        let protected = Inputs::open(paths)?;
        if let Some(plan) = &second {
            plan.paths(backend)?;
        }
        let runtime = registration::cleanup_runtime(backend, run, &post)?;
        let product = product_root(backend, &runtime)?;
        let handoff = document::read(&original.join("handoff.json"))?;
        let api_url = document::text(&handoff["api_url"], "原 API 地址")?.to_owned();
        common::endpoint(&api_url)?;
        let frame = Self {
            request,
            manifest,
            saved,
            marker,
            post,
            runtime,
            product,
            api_url,
            controller,
            protected,
            second,
        };
        frame.check()?;
        Ok(frame)
    }

    fn check(&self) -> Result<()> {
        self.protected.check()?;
        let (saved, controller) = evidence::current(self.request)?;
        if saved != self.saved || controller != self.controller {
            return Err("API 后继阶段的实际控制器或账本变化".into());
        }
        Ok(())
    }

    fn incident_closed(&self) -> Result<Value> {
        let worker_url = document::text(&self.runtime["worker_ready_url"], "原 Worker 就绪地址")?;
        let directory = self
            .second
            .as_ref()
            .map_or(self.post.runtime.as_path(), |plan| {
                plan.runtime_directory.as_path()
            });
        let incident = controller::observe_windows_api_incident(
            directory,
            &self.runtime,
            &self.api_url,
            worker_url,
        )?;
        self.check_incident(&incident)?;
        Ok(incident)
    }

    fn check_incident(&self, incident: &Value) -> Result<()> {
        if let Some(plan) = &self.second {
            plan.check_incident(incident)
        } else if *incident != self.marker.incident {
            Err("原 API 事故身份或失败收据不同于标记恢复依据".into())
        } else {
            Ok(())
        }
    }

    fn start(
        &self,
        check_closed: &dyn Fn() -> Result<()>,
        check_live: &dyn Fn(&Intent, &Value) -> Result<()>,
    ) -> Result<Value> {
        let (backend, run) = (self.request.backend, self.request.run_dir);
        let root = evidence::directory(run);
        let number = self.request.attempt.ok_or("后继 API 阶段序号缺失")?;
        let directory = evidence::attempt_directory(run, number);
        let receipt_directory = if self.second.is_some() {
            &directory
        } else {
            &root
        };
        guard::absent(&receipt_directory.join("intent.json"))?;
        guard::absent(&receipt_directory.join("published.json"))?;
        let observed = self.diagnose(Phase::BeforeStart, self.incident_closed())?;
        let incident = if self.second.is_some() {
            &observed
        } else {
            &self.marker.incident
        };
        common::ensure_directory(backend, &root, "API 后继根目录")?;
        fs::create_dir(&directory)?;
        prepare_frozen_runtime(backend, &self.product, &self.post.runtime, &directory)?;
        let frozen = self.hold_contract(&directory)?;
        let reservation = start::reserve(
            Role::Api,
            &self.product,
            &directory,
            &self.post.request["api_environment"],
        )?;
        let token = reservation.id().to_owned();
        let mut value = json!({"format_version":1,"kind":"devex-clone-api-successor-intent",
            "attempt":number,"registration":self.post.descriptor,"marker":self.marker.result,
            "runtime_directory":directory,"token":token,"executor":evidence::executor(&self.controller["identity"])?,
            "incident":incident});
        let second = self.second.as_ref().map(|plan| {
            value["format_version"] = json!(2);
            value["predecessor"] = plan.predecessor.clone();
            value["request"] = plan.request.clone();
            (plan, number)
        });
        let path = receipt_directory.join("intent.json");
        document::publish_new(&path, &value, "API 后继启动意图")?;
        let intent = Intent::load(self.request, &self.saved, second)?;
        let ready_url = format!("{}/readyz", self.api_url);
        self.producer(&intent, &ready_url)?.intent()?;
        check_closed()?;
        frozen.check()?;
        intent.check(backend)?;
        if self.incident_closed()? != intent.value["incident"] {
            return Err("启动前原 API 事故观察不同于已保存意图".into());
        }
        let started = self.diagnose(Phase::Start, reservation.start(60.0))?;
        self.diagnose(
            Phase::AfterStart,
            (|| {
                frozen.check()?;
                self.publish(&intent, &started["identity"], check_live)
            })(),
        )
    }

    fn reconcile(
        &self,
        intent: &Intent,
        identity: &Value,
        check_live: &dyn Fn(&Intent, &Value) -> Result<()>,
    ) -> Result<Value> {
        let backend = self.request.backend;
        let environment = &self.post.request["api_environment"];
        if intent.value["registration"] != self.post.descriptor
            || intent.value["marker"] != self.marker.result
        {
            return Err("待核对的 API 后继不属于当前同一事故与标记代次".into());
        }
        self.check_incident(&intent.value["incident"])?;
        let frozen = self.hold_contract(&intent.directory)?;
        check_live(intent, identity)?;
        intent.check(backend)?;
        let completed = controller::reconcile_completed_bound(
            Role::Api,
            &self.product,
            &intent.directory,
            environment,
            &intent.identity,
            "start",
            Some(&intent.token),
        )?;
        let archived = controller::archived_start_bound(
            Role::Api,
            &self.product,
            &intent.directory,
            environment,
            &intent.identity,
            &intent.token,
        )?
        .filter(|archive| archive["outcome"] == "succeeded")
        .ok_or("原 API 启动后继仍未知或失败，保留现场")?;
        if completed
            .as_ref()
            .is_some_and(|value| *value != archived["result"])
        {
            return Err("原 API 启动完成记录与归档后像不同".into());
        }
        frozen.check()?;
        self.publish(intent, &archived["result"]["identity"], check_live)
    }

    fn hold_contract(&self, directory: &Path) -> Result<Inputs> {
        let paths = ["runtime.json", "binaries.json"].map(|name| directory.join(name));
        let protected = Inputs::open(paths.clone())?;
        for path in paths {
            let name = path.file_name().ok_or("API 后继合同缺少固定文件名")?;
            let original = common::binding(&self.post.runtime.join(name))?;
            let current = common::binding(&path)?;
            if original["sha256"] != current["sha256"] || original["bytes"] != current["bytes"] {
                return Err("API 后继运行合同或二进制清单不同于原冻结字节".into());
            }
        }
        protected.check()?;
        Ok(protected)
    }

    fn producer<'b>(&'b self, intent: &'b Intent, ready_url: &'b str) -> Result<Producer<'b>> {
        Producer::new(
            self.request.backend,
            &intent.directory,
            &self.runtime,
            Role::Api,
            &intent.token,
            ready_url,
        )
    }

    fn live_identity(&self, intent: &Intent) -> Result<Value> {
        let backend = self.request.backend;
        let tree_path = common::common_file(
            backend,
            &intent.directory.join("api-tree.json"),
            "API 后继的原预留进程树",
        )?;
        let protected = Inputs::open([tree_path.clone(), intent.directory.join("api.json")])?;
        let observed = guard::live_api(backend, &self.product, &intent.directory, &self.runtime)?;
        let tree = document::read(&tree_path)?;
        if tree["operation_id"] != intent.token || tree["process"] != observed["identity"] {
            return Err("实际 API 进程树不属于原预留或进程身份不同".into());
        }
        listener(&self.api_url, &observed["identity"])?;
        crate::recovery::clone::native::target_verify::authority::require_closed(document::text(
            &self.runtime["worker_ready_url"],
            "原 Worker 就绪地址",
        )?)?;
        protected.check()?;
        Ok(observed["identity"].clone())
    }

    fn check_active(
        &self,
        intent: &Intent,
        identity: &Value,
        target: &Target,
        owner: &Generation,
        observation: &Path,
    ) -> Result<()> {
        if self.live_identity(intent)? != *identity {
            return Err("API 后继实际进程身份不同于原预留启动结果".into());
        }
        target.check_generation_for_api_successor(owner, &self.api_url)?;
        Self::verify_markers(target, observation)?;
        if self.live_identity(intent)? != *identity {
            return Err("API 后继在资源核对期间变化".into());
        }
        Ok(())
    }

    fn verify_markers(target: &Target, observation: &Path) -> Result<()> {
        redis::verify_live_markers(
            &target.storage_inputs(observation),
            &target.storage.value["redis"],
            &target.initialized["redis"],
            &SystemObserver,
        )
    }

    fn publish(
        &self,
        intent: &Intent,
        identity: &Value,
        check_live: &dyn Fn(&Intent, &Value) -> Result<()>,
    ) -> Result<Value> {
        let backend = self.request.backend;
        let guard = ProcessGuard::acquire_existing_named(&intent.directory, "api-control.guard")?;
        let check = || {
            guard.check()?;
            check_live(intent, identity)
        };
        check()?;
        intent.check(backend)?;
        let ready_url = format!("{}/readyz", self.api_url);
        self.producer(intent, &ready_url)?.started(identity)?;
        evidence::publish(
            self.request,
            intent,
            &self.post.descriptor,
            identity,
            &check,
        )
    }
}
