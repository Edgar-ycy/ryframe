//! 同一控制器先停 API，再让固定 Worker 排空，最后核对闭口。
use super::{Frame, PAIR, Role, control, drain};
use crate::{
    Result,
    recovery::clone::native::{
        json as document, lifecycle::stage::failure_type, resources::common,
        target_verify::storage::StorageInputs,
    },
};
use serde_json::{Value, json};
use std::{error::Error, path::Path};

use super::super::{
    protocol::guards::{close as model, result as result_model},
    runtime_model,
};

#[path = "close/checkpoints.rs"]
mod checkpoints;
#[path = "close/recovered.rs"]
mod recovered;
#[path = "close/stability.rs"]
mod stability;
use checkpoints::{Checkpoints, Prior};

pub(super) fn authorize_stop(frame: &Frame<'_>, phase: usize) -> Result<(Value, Vec<Value>)> {
    Checkpoints::new(frame).authorize_stop(phase)
}

pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    run: &Path,
    number: u64,
    handoff: &Value,
    target: &StorageInputs<'_>,
    fixed: &dyn Fn() -> Result<()>,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<Value> {
    let frame = Frame::load(
        backend,
        run,
        "status",
        PAIR,
        number,
        std::process::id(),
        Some(handoff),
    )?;
    let session = Session {
        frame: &frame,
        target,
        fixed,
        checkpoint,
    };
    let value = session.execute()?;
    frame.check()?;
    Ok(value)
}

struct Session<'a, 'b, 'c> {
    frame: &'a Frame<'b>,
    target: &'a StorageInputs<'c>,
    fixed: &'a dyn Fn() -> Result<()>,
    checkpoint: &'a dyn Fn() -> Result<()>,
}

impl Session<'_, '_, '_> {
    fn execute(&self) -> Result<Value> {
        self.fixed()?;
        let ledger = Checkpoints::new(self.frame);
        let (mut value, source) = match ledger.previous()? {
            Prior::Fresh => (self.first(false)?, None),
            Prior::Missing => (self.first(true)?, None),
            Prior::Resume { value, binding } => (self.resume(value)?, Some(binding)),
        };
        ledger.publish(&value, source.as_ref())?;
        if value["phase"] == "running" {
            self.stop_api(&mut value)?;
            if value["phase"] == "running" || value["reason"] == "post_api_stop_observation_failed"
            {
                return self.result(value);
            }
            ledger.publish(&value, None)?;
        }
        if value["phase"] == "api_stopped" {
            self.drain(&mut value)?;
            if value["phase"] == "api_stopped" {
                return self.result(value);
            }
            ledger.publish(&value, None)?;
        }
        if value["phase"] == "drained" {
            self.stop_worker(&mut value)?;
            if value["phase"] == "drained" {
                return self.result(value);
            }
            ledger.publish(&value, None)?;
        }
        self.finalize(&mut value)?;
        let value = self.result(value)?;
        if value["phase"] == "verified" {
            ledger.publish(&value, None)?;
        }
        Ok(value)
    }

    fn result(&self, value: Value) -> Result<Value> {
        let frame = self.frame;
        result_model::validate(
            &value,
            &frame.runtime.runtime,
            &frame.runtime.handoff,
            &frame.handoff,
            false,
        )?;
        Ok(value)
    }

    fn fixed(&self) -> Result<()> {
        self.frame.check()?;
        (self.fixed)()?;
        self.frame.check()
    }

    fn snapshot(&self) -> Result<Value> {
        self.frame.check()?;
        let check = || {
            self.frame.protected.checkpoint()?;
            self.frame.guard.check_named(
                &self.frame.run.join("seed-runtime"),
                "runtime-control.guard",
            )?;
            (self.checkpoint)()
        };
        let value = drain::observe(self.target, &check)?;
        self.frame.check()?;
        Ok(value)
    }

    fn status(&self) -> Result<Value> {
        role_result(control(self.frame, "status", PAIR)?)
    }

    fn stop(&self, role: Role, kind: &str) -> Result<Value> {
        let value = role_result(control(self.frame, "stop", &[role])?)?;
        self.require(&value, kind, &Value::Null)?;
        Ok(value)
    }

    fn require(&self, value: &Value, kind: &str, identities: &Value) -> Result<()> {
        model::require(
            value,
            kind,
            &self.frame.runtime.runtime,
            &self.frame.runtime.handoff,
            identities,
        )
    }

    fn require_current(&self, kind: &str, identities: &Value) -> Result<Value> {
        let value = self.status()?;
        self.require(&value, kind, identities)?;
        Ok(value)
    }

    fn recorded(&self) -> Result<Value> {
        let frame = self.frame;
        let mut identities = serde_json::Map::new();
        for role in PAIR {
            let name = role.name();
            let path = frame.runtime.runtime.join(format!("{name}.json"));
            let binding = common::binding(&common::common_file(
                frame.backend,
                &path,
                "原角色进程收据",
            )?)?;
            let value = common::read_bound(frame.backend, &binding, "原角色进程收据")?;
            document::exact(
                &value,
                &["format_version", "role", "scope_id", "identity"],
                "原角色进程收据",
            )?;
            if value["format_version"] != 1
                || value["role"] != name
                || value["scope_id"] != frame.runtime.handoff["scope_id"]
            {
                return Err("关闭进程收据不同于原角色和作用域".into());
            }
            identities.insert(
                name.into(),
                common::validate_process_identity(&value["identity"])?,
            );
        }
        Ok(Value::Object(identities))
    }

    fn first(&self, missing: bool) -> Result<Value> {
        let observed = self.status()?;
        let identities = self.recorded()?;
        let phase = if missing {
            self.require(&observed, "observed", &identities)?;
            if self.require(&observed, "fixed", &identities).is_ok() {
                "running"
            } else {
                self.require(&observed, "draining", &identities)?;
                "api_stopped"
            }
        } else {
            self.require(&observed, "fixed", &identities)?;
            "running"
        };
        let frame = self.frame;
        Ok(
            json!({"format_version":1,"kind":"devex-clone-seed-close","status":"needs_reconciliation",
            "phase":phase,"handoff":frame.handoff,"runtime_directory":frame.runtime.runtime,
            "scope_id":frame.runtime.handoff["scope_id"],"process_identities":identities,"runtime_status":observed,
            "api_stop":null,"drain":null,"worker_stop":null,"final_snapshot":null,"last_snapshot":null,
            "terminal_non_success":null,"stopped_evidence":null,"worker_preserved":true,"reason":null,
            "error_type":null,"outbox_drained":false,"direct_remote_writes":0,"worker_drain_writes":true,
            "restore_qualified":false}),
        )
    }

    fn resume(&self, value: Value) -> Result<Value> {
        resume_observed(
            value,
            self.status()?,
            &self.frame.runtime.runtime,
            &self.frame.runtime.handoff,
        )
    }

    fn stop_api(&self, value: &mut Value) -> Result<()> {
        let identities = value["process_identities"].clone();
        match self.stop(Role::Api, "api-stopped") {
            Ok(receipt) => {
                value["phase"] = json!("api_stopped");
                value["api_stop"] = receipt;
                clear_reason(value);
                match self.require_current("draining", &identities) {
                    Ok(observed) => value["runtime_status"] = observed,
                    Err(error) => partial(
                        value,
                        "post_api_stop_observation_failed",
                        Some(error.as_ref()),
                    ),
                }
            }
            Err(error) => {
                let observed = self.require_current("observed", &identities)?;
                if self.require(&observed, "fixed", &identities).is_ok() {
                    value["runtime_status"] = observed;
                    partial(value, "api_stop_failed", Some(error.as_ref()));
                } else {
                    self.require(&observed, "draining", &identities)?;
                    api_stopped(value, observed, Value::Null);
                }
            }
        }
        Ok(())
    }

    fn drain(&self, value: &mut Value) -> Result<()> {
        let identities = value["process_identities"].clone();
        let status = || self.require_current("draining", &identities).map(|_| ());
        match stability::observe(|| self.snapshot(), status) {
            Ok(stability::Outcome::Drained(evidence)) => {
                value["phase"] = json!("drained");
                last_snapshot(value, &evidence["last_snapshot"]);
                value["drain"] = evidence;
            }
            Ok(stability::Outcome::TimedOut(snapshot)) => {
                status()?;
                last_snapshot(value, &snapshot);
                partial(value, "drain_timeout", None);
                value["error_type"] = json!("DrainTimeout");
            }
            Err(error) => {
                status()?;
                partial(value, "drain_observation_failed", Some(error.as_ref()));
            }
        }
        Ok(())
    }

    fn stop_worker(&self, value: &mut Value) -> Result<()> {
        let identities = value["process_identities"].clone();
        if let Err(error) = self
            .fixed()
            .and_then(|_| self.require_current("draining", &identities))
        {
            self.require_current("draining", &identities)?;
            partial(value, "guard_failed", Some(error.as_ref()));
            return Ok(());
        }
        match self.stop(Role::Worker, "worker-stopped") {
            Ok(receipt) => worker_stopped(value, receipt),
            Err(error) => {
                let observed = self.status()?;
                if self.require(&observed, "draining", &identities).is_ok() {
                    partial(value, "worker_stop_failed", Some(error.as_ref()));
                } else {
                    self.require(&observed, "closed", &identities)?;
                    worker_stopped(value, Value::Null);
                    partial(
                        value,
                        "worker_stop_result_unavailable",
                        Some(error.as_ref()),
                    );
                }
            }
        }
        Ok(())
    }

    fn finalize(&self, value: &mut Value) -> Result<()> {
        let mut snapshot = Value::Null;
        let result = (|| {
            self.require_current("closed", &value["process_identities"])?;
            snapshot = self.snapshot()?;
            self.fixed()?;
            self.require_current("closed", &value["process_identities"])?;
            runtime_model::stopped(&self.frame.runtime.runtime, &self.frame.runtime.handoff)
        })();
        match result {
            Ok(evidence) => {
                last_snapshot(value, &snapshot);
                value["stopped_evidence"] = evidence;
                value["final_snapshot"] = snapshot;
            }
            Err(error) => {
                keep_failed_final_snapshot(value, &snapshot);
                partial(value, "final_verification_failed", Some(error.as_ref()));
                return Ok(());
            }
        }
        if model::drained(&value["final_snapshot"])? {
            value["status"] = json!("seed_runtime_closed");
            value["phase"] = json!("verified");
            value["outbox_drained"] = Value::Bool(true);
            clear_reason(value);
        } else {
            partial(value, "final_state_not_drained", None);
        }
        Ok(())
    }
}

fn resume_observed(
    mut value: Value,
    observed: Value,
    runtime: &Path,
    handoff: &Value,
) -> Result<Value> {
    let require = |kind| {
        model::require(
            &observed,
            kind,
            runtime,
            handoff,
            &value["process_identities"],
        )
    };
    match value["phase"].as_str() {
        Some("running") => {
            if require("fixed").is_err() {
                require("draining")?;
                api_stopped(&mut value, observed, Value::Null);
            }
        }
        Some("api_stopped") => require("draining")?,
        Some("drained") => {
            if require("draining").is_err() {
                require("closed")?;
                worker_stopped(&mut value, Value::Null);
                clear_reason(&mut value);
            }
        }
        Some("worker_stopped" | "verified") => require("closed")?,
        _ => return Err("关闭续作阶段无效".into()),
    }
    if value["status"] == "seed_runtime_closed" {
        value["phase"] = json!("worker_stopped");
        value["status"] = json!("needs_reconciliation");
        value["outbox_drained"] = Value::Bool(false);
    }
    Ok(value)
}

fn role_result(mut value: Value) -> Result<Value> {
    value
        .as_object_mut()
        .and_then(|object| object.remove("runtime"))
        .ok_or_else(|| "seed 角色关闭缺少固定运行结果".into())
}

fn partial(value: &mut Value, reason: &str, error: Option<&(dyn Error + 'static)>) {
    value["status"] = json!("needs_reconciliation");
    value["reason"] = json!(reason);
    value["error_type"] = json!(error.map(failure_type));
    value["outbox_drained"] = Value::Bool(false);
}

fn clear_reason(value: &mut Value) {
    value["reason"] = Value::Null;
    value["error_type"] = Value::Null;
}

fn api_stopped(value: &mut Value, observed: Value, receipt: Value) {
    value["phase"] = json!("api_stopped");
    value["runtime_status"] = observed;
    value["api_stop"] = receipt;
    clear_reason(value);
}

fn worker_stopped(value: &mut Value, receipt: Value) {
    value["phase"] = json!("worker_stopped");
    value["worker_stop"] = receipt;
    value["worker_preserved"] = Value::Bool(false);
}

fn last_snapshot(value: &mut Value, snapshot: &Value) {
    value["terminal_non_success"] = snapshot["terminal_non_success"].clone();
    value["last_snapshot"] = snapshot.clone();
}

fn keep_failed_final_snapshot(value: &mut Value, fresh: &Value) {
    if fresh.is_null() {
        let final_snapshot = value["final_snapshot"].clone();
        if !final_snapshot.is_null() {
            last_snapshot(value, &final_snapshot);
        }
    } else {
        // 旧结果仍保存在原阶段；当前终态计数要与这次实际观察一致。
        if value["final_snapshot"]["terminal_non_success"] != fresh["terminal_non_success"] {
            value["final_snapshot"] = Value::Null;
        }
        last_snapshot(value, fresh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_adopts_only_proven_role_exit_and_never_infers_drain_from_stopped_roles() {
        let runtime = Path::new("fixture-runtime");
        let handoff = json!({"scope_id":"seed"});
        let identity = json!({"pid":101,"started":"123","executable":"fixture.exe"});
        let stopped = json!({"state":"stopped","ready":false,"identity":null});
        let running = json!({"state":"running","ready":true,"identity":identity});
        let mut observed = json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":"status",
            "scope_id":"seed","runtime_directory":runtime,"producer_history":runtime.join("producer-history.json"),
            "processes":{"api":stopped,"worker":running}});
        let initial =
            json!({"phase":"running","process_identities":{"api":identity,"worker":identity}});
        let adopted =
            resume_observed(initial.clone(), observed.clone(), runtime, &handoff).unwrap();
        assert_eq!(adopted["phase"], "api_stopped");
        assert!(adopted["api_stop"].is_null());
        observed["processes"]["worker"] = stopped;
        assert!(resume_observed(initial.clone(), observed.clone(), runtime, &handoff).is_err());
        let mut drained = initial;
        drained["phase"] = json!("drained");
        let adopted = resume_observed(drained, observed, runtime, &handoff).unwrap();
        assert_eq!(adopted["phase"], "worker_stopped");
        assert!(adopted["worker_stop"].is_null());
    }

    #[test]
    fn close_final_failure_retains_prior_evidence_and_updates_fresh_observation() {
        let previous = json!({"terminal_non_success":{"dead":1}});
        let current = json!({"terminal_non_success":{"dead":2}});
        let stopped = json!({"history":{"sha256":"fixed"}});
        let mut value = json!({"final_snapshot":previous,"stopped_evidence":stopped});
        keep_failed_final_snapshot(&mut value, &Value::Null);
        assert_eq!(value["final_snapshot"], previous);
        assert_eq!(value["last_snapshot"], previous);
        assert_eq!(value["stopped_evidence"], stopped);
        keep_failed_final_snapshot(&mut value, &current);
        assert!(value["final_snapshot"].is_null());
        assert_eq!(value["last_snapshot"], current);
        assert_eq!(
            value["terminal_non_success"],
            current["terminal_non_success"]
        );
        assert_eq!(value["stopped_evidence"], stopped);
    }
}
