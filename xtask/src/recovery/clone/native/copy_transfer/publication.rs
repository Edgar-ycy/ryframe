//! 完整复制的结果在目标和账本目录锁释放后发布。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use time::OffsetDateTime;

use super::super::{
    copy_ledger::session::Mode, json as document, resources::common, target_verify::publication,
};
use super::{authority::Stage, record::Prepared, steps::Outcome};
use crate::Result;

pub(super) struct Attempt {
    mode: Mode,
    file: PathBuf,
}

impl Attempt {
    pub(super) fn new(mode: Mode, copy: &Path) -> Self {
        let file = if mode == Mode::New {
            copy.join("failure.json")
        } else {
            copy.join(format!(
                "continuation-{}.json",
                uuid::Uuid::new_v4().simple()
            ))
        };
        Self { mode, file }
    }

    pub(super) fn publish(
        &self,
        stage: &Stage<'_>,
        prepared: &Prepared,
        outcome: &Outcome,
        ledger: &Value,
    ) -> Result<Value> {
        stage.check()?;
        self.publish_bound(stage.backend, &stage.copy, prepared, outcome, ledger)
    }

    pub(super) fn publish_bound(
        &self,
        backend: &Path,
        copy: &Path,
        prepared: &Prepared,
        outcome: &Outcome,
        ledger: &Value,
    ) -> Result<Value> {
        prepared.check()?;
        let result = value(prepared, outcome, ledger)?;
        if matches!(outcome, Outcome::Verified { .. }) {
            let path = copy.join("result.json");
            if self.mode == Mode::Resume && path.try_exists()? {
                let path = common::common_file(backend, &path, "原完整复制结果")?;
                if document::read(&path)? != result {
                    return Err("已有复制结果与当前完整后像不同，保留原证据".into());
                }
            } else {
                publication::publish(&path, &result)?;
            }
        }
        if self.mode == Mode::New {
            return Ok(result);
        }
        publication::publish(
            &self.file,
            &json!({
                "mode":self.mode_name(), "completed_at":completed_at(),
                "plan_sha256":prepared.record["plan_sha256"],
                "generation_sha256":prepared.record["generation_sha256"],
                "ledger":ledger, "result":result,
            }),
        )?;
        let mut reported = result;
        reported["continuation_receipt"] = json!(self.file);
        Ok(reported)
    }

    pub(super) fn failure<T>(
        &self,
        backend: &Path,
        copy: &Path,
        result: Result<T>,
        phase: &str,
    ) -> Result<T> {
        let Err(error) = result else {
            return result;
        };
        if self.file.parent() != Some(copy) {
            return Err(format!("{error}；失败收据目录不属于原复制窗口").into());
        }
        if let Err(check) = common::local_directory(backend, copy, "复制失败证据") {
            return Err(format!("{error}；失败证据目录核验失败：{check}").into());
        }
        let diagnostic = if self.mode == Mode::New {
            json!({
                "format_version":1,"kind":"devex-copy-failure","phase":phase,
                "status":"needs_reconciliation","error_type":"NativeCopyError","frames":[],
                "errno":null,"winerror":null,
                "automatic_retry":false,"automatic_resource_cleanup":false,
                "target_ready":false,"restore_success":false,
            })
        } else {
            json!({
                "mode":self.mode_name(),"status":"needs_reconciliation","error_type":"NativeCopyError",
                "automatic_retry":false,"automatic_resource_cleanup":false,
                "target_ready":false,"restore_success":false,
            })
        };
        match publication::publish(&self.file, &diagnostic) {
            Ok(()) => Err(error),
            Err(record) => Err(format!("{error}；复制失败位置证据保存失败：{record}").into()),
        }
    }

    fn mode_name(&self) -> &'static str {
        match self.mode {
            Mode::New => "run",
            Mode::Reconcile => "reconcile",
            Mode::Resume => "resume",
        }
    }
}

fn value(prepared: &Prepared, outcome: &Outcome, ledger: &Value) -> Result<Value> {
    if ledger["plan_sha256"] != prepared.record["plan_sha256"]
        || ledger["generation_sha256"] != prepared.record["generation_sha256"]
    {
        return Err("复制结果的原账本计划或代次不一致".into());
    }
    let unchanged_databases = match outcome {
        Outcome::Verified {
            unchanged_databases,
        } => unchanged_databases,
        Outcome::Reconciled(value) => return Ok(value.clone()),
    };
    let written = document::object(&ledger["steps"], "原账本完整步骤")?.len();
    let valid = match written {
        0 => ledger["status"] == "partial",
        _ => ledger["status"] == "ledger_evidence_complete",
    };
    if !valid {
        return Err("复制账本没有通过对应收尾，不能发布数据步骤成功".into());
    }
    Ok(json!({
        "status":"data_steps_verified",
        "plan_sha256":prepared.record["plan_sha256"],
        "generation_sha256":prepared.record["generation_sha256"],
        "source_export_sha256":prepared.record["source_export"]["sha256"],
        "fresh_target_sha256":prepared.record["initialized"]["sha256"],
        "written_steps":written,
        "unchanged_databases":unchanged_databases.len(),
        "unchanged_database_images":unchanged_databases,
        "worker_stopped":true, "target_ready":false, "restore_success":false,
        "pending_target_actions":prepared.plan["pending_target_actions"],
        "requires_final_live_verification":true,
    }))
}

fn completed_at() -> String {
    crate::utc_timestamp::rfc3339_seconds_or_micros(OffsetDateTime::now_utc())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/publication.rs"]
mod tests;
