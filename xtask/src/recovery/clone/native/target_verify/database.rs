use std::{collections::BTreeMap, fs, path::Path};

use serde_json::{Value, json};

use super::super::{digest, export::transport::Transport, json as json_io};
use super::{
    input::{TargetContext, request::KEYS},
    storage::StorageInputs,
};
use crate::Result;

mod binding;
pub(crate) mod evidence;
pub(crate) mod model;

pub(crate) struct Captured {
    pub(crate) receipt: Value,
    pub(crate) observations: BTreeMap<String, Value>,
    pub(crate) evidence_files: Vec<Value>,
}

pub(crate) fn capture_with<T: Transport>(
    context: &TargetContext,
    transport: &T,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<Captured> {
    capture_at(
        &context.storage_inputs(),
        &context.observation_dir.join("inventory"),
        transport,
        checkpoint,
    )
}

pub(crate) fn capture_at<T: Transport>(
    context: &StorageInputs<'_>,
    directory: &Path,
    transport: &T,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<Captured> {
    checkpoint()?;
    fs::create_dir(directory)?;
    let mut capture = Capture {
        context,
        transport,
        checkpoint,
        directory,
        evidence: evidence::Evidence::default(),
        artifacts: BTreeMap::new(),
        stage: "inputs",
    };
    let result = capture.collect();
    if let Err(error) = &result {
        let mut failure = json!({
            "format_version": 1, "status": "side_inventory_failed", "stage": capture.stage,
            "error_type": "RustError", "remote_writes": 0,
            "target_ready": false, "clone_verified": false,
        });
        if let Some(error) = error.downcast_ref::<evidence::VerificationError>() {
            failure["error_type"] = json!("DatabaseVerificationError");
            failure["database_verification"] = error.details.clone();
        }
        json_io::write_new(&directory.join("failure.json"), &failure)?;
    }
    result
}

struct Capture<'a, T, F> {
    context: &'a StorageInputs<'a>,
    transport: &'a T,
    checkpoint: &'a F,
    directory: &'a Path,
    evidence: evidence::Evidence,
    artifacts: BTreeMap<String, Value>,
    stage: &'static str,
}

#[derive(PartialEq)]
struct Round {
    raw: BTreeMap<String, Value>,
    observations: BTreeMap<String, Value>,
}

impl<T: Transport, F: Fn() -> Result<()>> Capture<'_, T, F> {
    fn collect(&mut self) -> Result<Captured> {
        self.stage = "binding_before";
        let before = self.binding()?;
        self.evidence
            .write(self.directory, "binding-before.json", &before)?;
        self.stage = "inventory_before";
        let first = self.observe("before", &before)?;
        self.stage = "inventory_after";
        let second = self.observe("after", &before)?;
        if first != second {
            return Err("两次完整库存不同，不能声明只读稳定".into());
        }
        self.stage = "binding_after";
        let after = self.binding()?;
        if before != after {
            return Err("采集期间来源、工具、配置或物理绑定变化".into());
        }
        self.evidence
            .write(self.directory, "binding-after.json", &after)?;
        self.publish(&before, second)
    }

    fn binding(&self) -> Result<Value> {
        (self.checkpoint)()?;
        let value = binding::capture(self.context)?;
        (self.checkpoint)()?;
        Ok(value)
    }

    fn observe(&mut self, phase: &str, expected: &Value) -> Result<Round> {
        let databases = model::databases(self.context)?;
        self.verify_databases(&databases)?;
        let backend = self.context.execution_backend()?;
        let mut round = Round {
            raw: BTreeMap::new(),
            observations: BTreeMap::new(),
        };
        let mut schemas = None;
        for (key, _, _) in KEYS {
            if self.binding()? != *expected {
                return Err("调用库存 CLI 前来源或配置已变化".into());
            }
            let name = format!("{phase}-target-{key}.json");
            let path = self.directory.join(&name);
            let port = evidence::Port {
                context: self.context,
                transport: self.transport,
                checkpoint: self.checkpoint,
                directory: self.directory,
            };
            self.evidence.inventory(&port, &path)?;
            let raw = json_io::read(&path)?;
            let owners = self.owners(&databases[key])?;
            let observed = model::observation(
                &backend,
                &raw,
                &databases[key],
                self.context.target_scope()?,
                &owners,
            )?;
            let fingerprints = model::fingerprints(&raw)?;
            if schemas.as_ref().is_some_and(|value| *value != fingerprints) {
                return Err("四目标库存来自不同编译 schema".into());
            }
            schemas = Some(fingerprints);
            self.artifacts
                .insert(name.clone(), evidence::descriptor(&path)?);
            self.evidence.track(self.directory, &name)?;
            round.raw.insert(key.into(), raw);
            round.observations.insert(key.into(), observed);
        }
        self.verify_databases(&databases)?;
        Ok(round)
    }

    fn owners(&mut self, database: &Value) -> Result<Vec<Value>> {
        let port = evidence::Port {
            context: self.context,
            transport: self.transport,
            checkpoint: self.checkpoint,
            directory: self.directory,
        };
        let response = self.evidence.mysql(&port, database, "ownership")?;
        let rows = model::owners(&response)
            .map_err(|_| evidence::mismatch("ownership", database, &response))?;
        if rows
            != model::expected_owners(self.context.target_scope()?, database["kind"] == "combined")
        {
            return Err(evidence::mismatch("ownership", database, &response));
        }
        Ok(rows)
    }

    fn verify_databases(&mut self, databases: &BTreeMap<String, Value>) -> Result<()> {
        for (key, _, _) in KEYS {
            let db = &databases[key];
            let port = evidence::Port {
                context: self.context,
                transport: self.transport,
                checkpoint: self.checkpoint,
                directory: self.directory,
            };
            let response = self.evidence.mysql(&port, db, "identity")?;
            let identity = response.split('\t').collect::<Vec<_>>();
            if identity
                != [
                    json_io::text(&db["server_uuid"], "数据库 UUID")?,
                    json_io::text(&db["database"], "物理库名")?,
                ]
            {
                return Err(evidence::mismatch("identity", db, &response));
            }
            self.owners(db)?;
        }
        Ok(())
    }

    fn publish(&mut self, before: &Value, round: Round) -> Result<Captured> {
        for (name, expected) in &self.artifacts {
            if evidence::descriptor(&self.directory.join(name))? != *expected {
                return Err("已采集库存文件缺失或被修改".into());
            }
        }
        let images = model::physical_images(self.context, &round.observations)?;
        let receipt = json!({
            "format_version": 1, "status": "side_inventory_captured", "side": "target",
            "scope_id": self.context.target_scope()?, "binding_sha256": digest::plan_hash(before)?,
            "keys": KEYS.map(|(key, _, _)| key), "inventories": self.artifacts,
            "observations": round.observations, "remote_writes": 0,
            "producer_stopped_proven": false, "fresh_target_proven": false,
            "target_ready": false, "clone_verified": false, "restore_qualified": false,
        });
        self.stage = "publish";
        (self.checkpoint)()?;
        self.evidence.files(self.directory)?;
        self.evidence
            .write(self.directory, "inventory.json", &receipt)?;
        let path = self.directory.join("inventory.json");
        let published = evidence::descriptor(&path)?;
        let result = self.evidence.files(self.directory).and_then(|files| {
            (self.checkpoint)()?;
            Ok(Captured { receipt: json!({"path": path, "bytes": published["bytes"], "sha256": published["sha256"]}), observations: images, evidence_files: files })
        });
        if result.is_err() && evidence::descriptor(&path)? == published {
            fs::remove_file(path)?;
        }
        result
    }
}
