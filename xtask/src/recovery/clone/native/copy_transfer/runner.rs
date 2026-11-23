//! 正式复制阶段共用外层生命周期、目标锁及一次持久账本会话。
use std::{fs, path::Path};

use serde_json::{Value, json};

use super::super::{
    cache::handoff::locks::Storage,
    copy_ledger::session,
    copy_owner::Owner,
    export::transport::SystemTransport,
    json as document,
    lifecycle::{
        lock::ProcessGuard,
        stage::{StageRequest, StageRunner},
    },
    resources::common,
    target_verify::{database::SourceBinding, guard::Generation, storage::StorageInputs},
};
use super::{
    authority::{Source, Stage},
    declaration::{Declaration, compose},
    observe_database::{Baseline, Side, Window},
    observe_source::{self, ObserveSource, Selection},
    proofs::Proofs,
    publication::Attempt,
    record::{Continuation, Facts, Files, Prepared},
    steps,
    target_port::Target,
};
use crate::{Result, cli::CloneSide};

mod io;

pub(crate) struct Runner;

impl StageRunner for Runner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if request.attempt.is_some() {
            return Err("复制只读预检必须在申请阶段序号之前执行".into());
        }
        Stage::load(request)?.preflight().map(|_| ())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        if request.attempt.is_none() {
            return Err("复制必须由持锁的正式阶段执行".into());
        }
        let stage = Stage::load(request)?;
        let saved = stage.preflight()?;
        let storage = Storage::acquire(stage.backend, stage.run, &stage.manifest)?;
        let result = match saved {
            None => start(&stage, &storage),
            Some(saved) => continue_copy(&stage, &storage, &saved),
        };
        storage.finish(result)
    }
}

fn start(stage: &Stage<'_>, storage: &Storage) -> Result<Value> {
    fs::create_dir(&stage.copy)?;
    common::local_directory(stage.backend, &stage.copy, "新复制目录")?;
    let attempt = Attempt::new(stage.mode, &stage.copy);
    let mut phase = "prepare";
    let result = (|| {
        let operation = Operation::load(stage, storage, None)?;
        let data = operation.prepare(None, None)?;
        phase = "register-owner";
        let owner = operation.claim_owner()?;
        phase = "transfer";
        operation.transfer(owner, &data, &attempt)
    })();
    attempt.failure(stage.backend, &stage.copy, result, phase)
}

fn continue_copy(stage: &Stage<'_>, storage: &Storage, saved: &Continuation) -> Result<Value> {
    let attempt = Attempt::new(stage.mode, &stage.copy);
    let result = (|| {
        let files = Files::bind(stage.backend, &stage.manifest, &stage.descriptor)?;
        with_continuation_owner(
            stage.backend,
            &stage.copy,
            &files,
            saved,
            |owner| {
                let operation = Operation::load(stage, storage, Some(owner))?;
                let data = operation.prepare(Some(saved), Some(owner))?;
                Ok((operation, data))
            },
            |owner, (operation, data)| operation.transfer(owner, &data, &attempt),
        )
    })();
    attempt.failure(stage.backend, &stage.copy, result, "continuation")
}

pub(super) fn with_continuation_owner<B, T>(
    backend: &Path,
    copy: &Path,
    files: &Files<'_>,
    saved: &Continuation,
    prepare: impl FnOnce(&Generation) -> Result<B>,
    transfer: impl FnOnce(Generation, B) -> Result<T>,
) -> Result<T> {
    saved.check()?;
    let owner = Generation::acquire_copy(Owner::register(
        backend,
        copy,
        files.source_export,
        files.initialized,
    )?)?;
    match saved.check().and_then(|()| prepare(&owner)) {
        Ok(prepared) => transfer(owner, prepared),
        Err(error) => owner.finish(Err(error)),
    }
}

struct Operation<'a> {
    stage: &'a Stage<'a>,
    storage: &'a Storage,
    files: Files<'a>,
    source: Source,
    target: Target,
    transport_plan: Value,
}

struct Bound {
    baseline: Baseline,
    declaration: Value,
    proofs: Proofs,
    prepared: Prepared,
}

impl<'a> Operation<'a> {
    fn load(
        stage: &'a Stage<'a>,
        storage: &'a Storage,
        owner: Option<&Generation>,
    ) -> Result<Self> {
        stage.check()?;
        storage.check()?;
        let files = Files::bind(stage.backend, &stage.manifest, &stage.descriptor)?;
        let source = Source::load(stage)?;
        let target = Target::open(stage.backend, &stage.manifest, &storage.directory, owner)?;
        let mut selected = target.request["target"].clone();
        for field in ["runtime_dir", "api_url"] {
            selected[field] = target.initialized["generation"]["selected"][field].clone();
        }
        selected["s3"] = json!({"configuration":target.configuration});
        let tools = json!({"mysql":source.request["tools"]["mysql"]});
        if tools != target.request["tools"]
            || selected["scope_id"] == source.request["target"]["scope_id"]
        {
            return Err("两侧复制必须使用同一 MySQL 工具和独立的目标 scope".into());
        }
        let transport_plan =
            json!({"source":source.request["target"],"target":selected,"tools":tools});
        let operation = Self {
            stage,
            storage,
            files,
            source,
            target,
            transport_plan,
        };
        operation.checkpoint(owner)?;
        Ok(operation)
    }

    fn checkpoint(&self, owner: Option<&Generation>) -> Result<()> {
        self.stage.check()?;
        self.storage.check()?;
        self.files.check()?;
        self.source.check_inputs()?;
        self.target.check_inputs()?;
        if let Some(owner) = owner {
            owner.checkpoint()?;
        }
        Ok(())
    }

    fn claim_owner(&self) -> Result<Generation> {
        self.checkpoint(None)?;
        Generation::acquire_copy(Owner::register(
            self.stage.backend,
            &self.stage.copy,
            self.files.source_export,
            self.files.initialized,
        )?)
    }

    fn prepare(&self, saved: Option<&Continuation>, owner: Option<&Generation>) -> Result<Bound> {
        self.checkpoint(owner)?;
        let proofs = self.proofs(saved)?;
        let window = self.database_window(saved.map(|saved| &saved.record["source_observation"]));
        let check = || self.checkpoint(owner);
        let baseline =
            window.baseline(&self.output("source-inventory"), &SystemTransport, &check)?;
        self.observe_source(&check)?;
        self.source.check_generation(self.stage)?;
        let declaration = compose(Declaration {
            export: &self.source.export,
            source: &baseline.source().observations,
            target: &self.target.request["target"],
            target_endpoint: &self.target.endpoint,
            initialized: baseline.initialized(),
            evidence: &proofs.descriptors,
            copy_id: document::text(&self.stage.manifest["id"], "复制运行 ID")?,
            stage: document::text(&self.stage.manifest["copy_stage"], "复制阶段")?,
            execution_backend: &self.source.execution,
        })?;
        let facts = Facts {
            source_generation: &self.source.frozen.generation,
            target_generation: &self.target.initialized["generation"],
            source_observation: &baseline.source_observation(),
            declaration: &declaration,
        };
        let prepared = match saved {
            Some(record) => self.files.hydrate(record, facts)?,
            None => self.files.prepare(facts)?,
        };
        self.checkpoint(owner)?;
        proofs.checkpoint()?;
        Ok(Bound {
            baseline,
            declaration,
            proofs,
            prepared,
        })
    }

    fn proofs(&self, saved: Option<&Continuation>) -> Result<Proofs> {
        let source = &self.source.export["evidence"]["generation-after"];
        if let Some(saved) = saved {
            return Proofs::open(
                &self.source.root,
                &saved.declaration["evidence"],
                source,
                self.files.initialized,
                &self.target.initialized,
            );
        }
        let stopped = self.target.verify_initial(&self.output("target-precopy"))?;
        Proofs::publish(
            self.stage.backend,
            &self.source.root,
            source,
            self.files.initialized,
            &self.target.initialized,
            &stopped,
        )
    }

    fn check_source_proofs(&self) -> Result<()> {
        self.source
            .frozen
            .check_proofs(self.stage.backend, self.files.source_export)
    }

    fn observe_source(&self, check: &(dyn Fn() -> Result<()> + Sync)) -> Result<()> {
        self.check_source_proofs()?;
        observe_source::observe(
            ObserveSource {
                backend: self.stage.backend,
                work: &self.stage.copy,
                directory: &self.output("source-objects"),
                export: self.files.source_export,
                captures: &self.source.frozen.captures,
                scope: document::text(&self.source.request["target"]["scope_id"], "源 scope")?,
                endpoint: &self.source.endpoint,
                region: &self.source.region,
                configuration: &self.source.configuration,
            },
            Selection::All {
                verify_proofs: &|| self.check_source_proofs(),
            },
            &SystemTransport,
            check,
        )?;
        self.check_source_proofs()
    }

    fn database_window<'s>(&'s self, frozen: Option<&'s Value>) -> Window<'s> {
        let (source_environment, _) = self.files.environments();
        let source = Side {
            storage: StorageInputs {
                backend: self.stage.backend,
                request: &self.source.request,
                environment: source_environment,
                observation_dir: &self.stage.copy,
            },
            binding: self.database_binding(
                CloneSide::Source,
                &self.source.request,
                &self.source.configuration,
            ),
        };
        let target = Side {
            storage: self.target.storage_inputs(&self.stage.copy),
            binding: self.database_binding(
                CloneSide::Target,
                &self.target.request,
                &self.target.configuration,
            ),
        };
        Window {
            source,
            target,
            export_inventory: &self.source.frozen.inventory,
            initialized: &self.target.initialized,
            frozen_source_observation: frozen,
        }
    }

    fn database_binding<'s>(
        &'s self,
        side: CloneSide,
        request: &'s Value,
        configuration: &'s Value,
    ) -> SourceBinding<'s> {
        SourceBinding {
            side,
            plan: Some(&self.transport_plan),
            selected: &self.transport_plan[side.as_str()],
            tools: &request["tools"],
            maintenance: &request["maintenance_build"],
            scope: &self.stage.scope,
            configuration: Some(configuration),
        }
    }

    fn output(&self, prefix: &str) -> std::path::PathBuf {
        let unique = uuid::Uuid::new_v4().simple().to_string();
        self.stage.copy.join(format!("{prefix}-{}", &unique[..8]))
    }

    fn transfer(&self, owner: Generation, bound: &Bound, attempt: &Attempt) -> Result<Value> {
        let execution = self.transfer_locked(&owner, bound);
        let mut published = None;
        owner.finish_publishing(execution, |(outcome, ledger)| {
            self.checkpoint(None)?;
            published = Some(attempt.publish(self.stage, &bound.prepared, outcome, ledger)?);
            self.checkpoint(None)
        })?;
        published.ok_or_else(|| "复制外层结果没有发布".into())
    }

    fn transfer_locked(
        &self,
        owner: &Generation,
        bound: &Bound,
    ) -> Result<(steps::Outcome, Value)> {
        let authority = owner.copy_session()?;
        let plan = steps::Plan::from_verified(&bound.prepared.plan)?;
        let window = self.database_window(Some(&bound.prepared.record["source_observation"]));
        let mut io = io::Live::open(self, bound, &window, owner, authority.0)?;
        run_guarded_steps(
            authority,
            &plan,
            self.stage.mode,
            &mut io,
            io::Live::verify_target_owners,
            || self.source.complete_source_window(),
        )
    }
}

pub(super) fn run_guarded_steps<I: steps::CopyIo>(
    (current, guard, lock): (&Owner, &ProcessGuard, &Value),
    plan: &steps::Plan<'_>,
    mode: session::Mode,
    io: &mut I,
    verify_target_owners: impl FnOnce(&I) -> Result<()>,
    complete_source_window: impl FnOnce() -> Result<()>,
) -> Result<(steps::Outcome, Value)> {
    verify_target_owners(io)?;
    session::run(current, guard, lock, mode, |ledger| {
        let execution = steps::run(plan, ledger, mode, io);
        let checked = complete_source_window();
        match (execution, checked) {
            (Ok(outcome), Ok(())) => Ok(outcome),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(after)) => {
                Err(format!("{error}；运行来源完整后验失败：{after}").into())
            }
        }
    })
}
