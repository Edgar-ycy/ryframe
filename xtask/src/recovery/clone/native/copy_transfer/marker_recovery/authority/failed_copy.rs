//! 绑定已关闭的失败复制核对与原本机控制器及完整账本。
use serde_json::{Value, json};

use super::{FailedMarkerJournal, Files, Inputs, Stage, common, hold, state};
use crate::{Result, recovery::clone::native::copy_owner::Owner};

pub(super) struct FailedCopy {
    pub(super) journal: FailedMarkerJournal,
    pub(super) bound: Inputs,
}

pub(super) fn is_failed_reconcile(row: &Value) -> bool {
    row["mode"] == "reconcile"
        && row["status"] == "failed"
        && row["error_type"] == "StageError"
        && row["finished_at"].is_string()
        && row["result"].is_null()
}

impl FailedCopy {
    pub(super) fn bind(
        stage: &Stage<'_>,
        row: &Value,
        files: &Files<'_>,
        journal: FailedMarkerJournal,
    ) -> Result<FailedCopy> {
        let number = row["number"].as_u64().ok_or("原失败核对缺少阶段序号")?;
        let session = journal.session_id()?;
        let uuid = session
            .strip_prefix("session-")
            .filter(|text| {
                text.len() == 32
                    && text
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            })
            .ok_or("原失败核对缺少已登记复制会话名")?;
        let run = stage.request.run_dir;
        let backend = stage.request.backend;
        let owner_path = files.root.join(format!("copy-owner-{uuid}.json"));
        let bound = hold(
            backend,
            &[state::controller_path(run, number), owner_path.clone()],
        )?;
        let mut running = row.clone();
        running["status"] = json!("running");
        for field in ["finished_at", "result", "error_type"] {
            running[field] = Value::Null;
        }
        let (_, controller) = state::controller_record(run, number, &running)?;
        let owner = Owner::read(
            backend,
            &files.root,
            &common::binding(&owner_path)?,
            files.source_export,
            files.initialized,
        )?;
        if owner.session_id() != session || owner.controller() != &controller["identity"] {
            return Err("原失败复制核对阶段与最后失败账本会话不是同一控制器".into());
        }
        bound.check()?;
        Ok(FailedCopy { journal, bound })
    }
}
