//! 按清单顺序提交参考恢复任务，每次发送前后复核原绑定。
use std::path::Path;

use serde_json::{Value, json};

use super::{model, native, object_write, text};
use crate::Result;
use native::{
    copy_transfer::mysql, export::transport::Transport, s3::PutRequest,
    target_verify::storage::StorageInputs,
};

pub(super) struct Writes<'a> {
    pub(super) storage: StorageInputs<'a>,
    pub(super) work: &'a Path,
    pub(super) backup_root: &'a Path,
    pub(super) plan: &'a Value,
    pub(super) manifest: &'a Value,
}

impl Writes<'_> {
    pub(super) fn run(
        &self,
        tasks: &Value,
        transport: &impl Transport,
        put: impl Fn(PutRequest) -> Result<Value>,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        for task in native::json::array(tasks, "参考恢复授权任务")? {
            checkpoint()?;
            let artifact = model::bound_artifact(&json!({
                "root":self.backup_root,"manifest":self.manifest,"relative":task["relative"],
            }))?;
            match task["kind"].as_str() {
                Some("database") => mysql::reference::write(
                    &self.storage,
                    &mysql::reference::Write {
                        backup_root: self.backup_root,
                        work: self.work,
                        task,
                        artifact: &artifact,
                        mysql: &self.plan["tools"]["mysql"],
                    },
                    transport,
                    checkpoint,
                )?,
                Some("object") => {
                    object_write::send_once(
                        &object_write::Create {
                            backend: self.storage.backend,
                            work: self.work,
                            artifact_root: self.backup_root,
                            task,
                            artifact: &artifact,
                            target_s3: &self.plan["target"]["s3"],
                            scope: text(&self.plan["target"]["scope_id"])?,
                        },
                        checkpoint,
                        &put,
                    )?;
                }
                _ => return Err("参考恢复授权任务类型无效".into()),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/source/evidence/reference_workflow/restore/writes_tests.rs"]
mod tests;
