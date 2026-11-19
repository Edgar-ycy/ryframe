//! 步骤编排只通过受保护的真实数据库、对象和 Redis 端口操作。
use std::sync::Mutex;

use super::super::{
    database_port::{self, DatabasePort},
    object_port::{self, ObjectPort},
    observe_database::Window,
    steps::{Basis, CopyIo, Kind, Observation, Step},
};
use super::{Bound, Operation};
use crate::{
    Result,
    recovery::clone::native::{
        copy_ledger::session::Session, copy_owner::Owner, export::transport::SystemTransport,
        json as document, target_verify::guard::Generation,
    },
};

pub(super) struct Live<'a> {
    checks: Checks<'a>,
    database: DatabasePort<'a, SystemTransport>,
    object: ObjectPort<'a, SystemTransport>,
}

struct Checks<'a> {
    operation: &'a Operation<'a>,
    bound: &'a Bound,
    owner: &'a Generation,
    serial: Mutex<()>,
}

impl Checks<'_> {
    fn request(&self) -> Result<()> {
        let _guard = self
            .serial
            .lock()
            .map_err(|_| "复制请求的共同保护检查中断")?;
        self.operation.checkpoint(Some(self.owner))?;
        self.bound.prepared.check()?;
        self.bound.proofs.checkpoint()
    }

    fn complete(&self) -> Result<()> {
        self.request()?;
        self.operation
            .source
            .check_generation(self.operation.stage)?;
        self.operation.target.check_generation(self.owner)?;
        self.request()
    }
}

pub(super) enum Evidence {
    Database(Box<database_port::Evidence>),
    Object(object_port::Evidence),
}

impl<'a> Live<'a> {
    pub(super) fn open(
        operation: &'a Operation<'a>,
        bound: &'a Bound,
        window: &'a Window<'a>,
        owner: &'a Generation,
        current: &'a Owner,
    ) -> Result<Self> {
        let prepared = &bound.prepared;
        let plan = &prepared.plan;
        let generation = document::text(&prepared.record["generation_sha256"], "复制代次")?;
        let stage = operation.stage;
        let source = &operation.source;
        let target = &operation.target;
        let database = DatabasePort::open(
            database_port::Bind {
                window,
                baseline: &bound.baseline,
                owner: current,
                copy_root: &stage.copy,
                export_root: &source.root,
                export: &source.export,
                declaration: &bound.declaration,
                plan,
                generation_sha256: generation,
            },
            &SystemTransport,
        )?;
        let object = ObjectPort::new(
            object_port::ObjectInputs {
                backend: stage.backend,
                target_configuration_root: &target.execution,
                work: &stage.copy,
                export_root: &source.root,
                source: object_port::Side {
                    scope: document::text(&plan["source_scope"], "复制源 scope")?,
                    endpoint: &source.endpoint,
                    region: &source.region,
                    configuration: &source.configuration,
                },
                target: object_port::Side {
                    scope: document::text(&plan["target_scope"], "复制目标 scope")?,
                    endpoint: &target.endpoint,
                    region: &target.region,
                    configuration: &target.configuration,
                },
                export: operation.files.source_export,
                captures: &source.frozen.captures,
                proof_files: &source.frozen.proof_files,
                plan_sha256: document::text(&plan["plan_sha256"], "复制计划")?,
                generation_sha256: generation,
            },
            &SystemTransport,
        )?;
        let checks = Checks {
            operation,
            bound,
            owner,
            serial: Mutex::new(()),
        };
        checks.complete()?;
        Ok(Self {
            checks,
            database,
            object,
        })
    }

    pub(super) fn verify_target_owners(&self) -> Result<()> {
        self.object
            .verify_target_owners(&|| self.checks.request())?;
        Ok(())
    }
}

impl CopyIo for Live<'_> {
    type Evidence = Evidence;

    fn checkpoint(&mut self, _: Option<&Step<'_>>) -> Result<()> {
        self.checks.complete()
    }

    fn check_resume_target(&mut self) -> Result<()> {
        self.checks.operation.target.check_redis(self.checks.owner)
    }

    fn basis(&mut self, step: &Step<'_>) -> Result<Basis> {
        match step.kind {
            Kind::Database => self.database.basis(step),
            Kind::Object => self.object.basis(step),
        }
    }

    fn observe_databases(&mut self, all: &[Step<'_>]) -> Result<Observation<Evidence>> {
        let observation = self.database.observe(all, &|| self.checks.request())?;
        Ok(Observation {
            images: observation.images,
            evidence: Evidence::Database(Box::new(observation.evidence)),
        })
    }

    fn observe_objects(&mut self, selected: &[&Step<'_>]) -> Result<Observation<Evidence>> {
        let checks = &self.checks;
        let observation = self.object.observe(selected, &|| checks.request())?;
        Ok(Observation {
            images: observation.images,
            evidence: Evidence::Object(observation.evidence),
        })
    }

    fn check_observation(&mut self, observation: &Observation<Evidence>) -> Result<()> {
        let images = &observation.images;
        match &observation.evidence {
            Evidence::Database(evidence) => self.database.check_observation(images, evidence),
            Evidence::Object(evidence) => self.object.check_observation(images, evidence),
        }
    }

    fn write(&mut self, step: &Step<'_>, session: &mut Session<'_>, basis: &Basis) -> Result<()> {
        let check = || self.checks.request();
        match step.kind {
            Kind::Database => self.database.write(step, session, basis, &check),
            Kind::Object => self.object.write(step, session, basis, &check),
        }
    }

    fn verify_complete(&mut self) -> Result<()> {
        self.checks.complete()?;
        let check = || self.checks.request();
        self.database.verify_source(&check)?;
        self.object
            .verify_source(&check, &|| self.checks.operation.check_source_proofs())?;
        self.database.verify_source(&check)?;
        let items = document::array(
            &self.checks.bound.prepared.plan["objects"],
            "复制目标完整对象",
        )?;
        self.object.verify_target(items, &check)?;
        self.checks
            .operation
            .target
            .check_redis(self.checks.owner)?;
        self.database.verify_target_complete(&check)?;
        self.checks.complete()
    }
}
