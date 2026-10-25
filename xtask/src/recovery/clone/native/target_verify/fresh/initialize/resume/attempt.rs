//! 单次续作保存原始失败、确认进度与安全OS属性；异常路径不推断命令是否执行。
use super::super::super::super::super::{export::transport::Transport, resources::common};
use super::super::super::super::resume::transition;
use super::super::super::{failure, receipts, registration::Registration};
use super::super::window::Window;
use crate::Result;
use serde_json::{Value, json};
use std::path::PathBuf;

pub(crate) struct Attempt {
    pub(in super::super) id: String,
    pub(in super::super) state: Value,
    pub(in super::super) session: Value,
    pub(in super::super) intent: PathBuf,
    pub(in super::super) started: PathBuf,
}
impl Attempt {
    pub(crate) fn new(reg: &Registration, state: Value) -> Result<Self> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let session = if state["mode"] == "inventory" {
            Value::Null
        } else {
            transition::execute(
                &reg.backend,
                &reg.target,
                "session-start",
                &json!({"state":state}),
            )?
        };
        Ok(Self {
            intent: reg
                .target
                .join(format!("resume-initialize-{id}.intent.json")),
            started: reg
                .target
                .join(format!("resume-initialize-{id}.started.json")),
            id,
            state,
            session,
        })
    }
    pub(in super::super) fn now(&self) -> String {
        chrono::Utc::now().to_rfc3339()
    }
    pub(in super::super) fn transition<T: Transport>(
        &self,
        w: &Window<'_, T>,
        kind: &str,
        args: &Value,
    ) -> Result<Value> {
        transition::execute(w.inputs.backend, w.inputs.observation_dir, kind, args)
    }
    pub(in super::super) fn command_paths<T: Transport>(
        &self,
        w: &Window<'_, T>,
        label: &str,
    ) -> Result<Vec<PathBuf>> {
        let prefix = format!("{label}-");
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(w.inputs.observation_dir)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or("命令收据文件名无效")?;
            if name.starts_with(&prefix) && name.ends_with(".command.json") {
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }
    pub(in super::super) fn confirmation<T: Transport>(
        &self,
        w: &Window<'_, T>,
        marker: Option<Value>,
    ) -> Result<Value> {
        let mut args = json!({"attempt":self.id,"at":self.now(),"intent":common::binding(&self.intent)?,"failure":self.state["failure"],
            "initialized":common::binding(&w.inputs.observation_dir.join("initialized.json"))?,"prepared_files":self.state["prepared_files"]});
        if self.state["mode"] != "inventory" {
            let fields = json!({"started":common::binding(&self.started)?,"original_prefix":self.state["completed"],"resumed_operations":self.session["completed"],
                "redis_marker_rebind":marker,"remote_write_operations":self.session["confirmed_writes"],"predecessor":self.state["predecessor"]});
            args.as_object_mut()
                .ok_or("续作确认无效")?
                .extend(fields.as_object().ok_or("迁移确认无效")?.clone());
        }
        Ok(args)
    }
    pub(in super::super) fn failed(
        &mut self,
        reg: &Registration,
        error: &(dyn std::error::Error + 'static),
    ) -> Result<()> {
        if !self.intent.try_exists()? {
            return Ok(());
        }
        let args = if self.state["mode"] == "inventory" {
            json!({"attempt":self.id,"at":self.now(),"intent":common::binding(&self.intent)?,"source_failure":self.state["failure"],"prepared_files":self.state["prepared_files"],"error_type":failure::category(error)})
        } else {
            let receipt = error.downcast_ref::<failure::Publication>().filter(|v| {
                v.path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .is_some_and(|n| n.ends_with(".command.json"))
            });
            if let Some(receipt) = receipt
                && self.session["active_operation"].is_object()
                && self.session["active_operation"]["index"].is_u64()
            {
                let length = receipt.path.to_string_lossy().chars().count();
                self.session["active_operation"]["phase"] = json!("command-receipt-publish");
                self.session["active_operation"]["receipt_path_length"] = json!(length);
                self.session["active_operation"]["receipt_path_limit_risk"] = json!(length >= 260);
            }
            json!({"attempt":self.id,"at":self.now(),"intent":common::binding(&self.intent)?,"source_failure":self.state["failure"],"prepared_files":self.state["prepared_files"],
                "started":if self.started.is_file(){common::binding(&self.started)?}else{Value::Null},"error_type":failure::category(error),
                "confirmed_operations":self.session["completed"],"remote_write_operations_confirmed":self.session["confirmed_writes"],"active_write":self.session["active_write"],
                "active_operation":self.session["active_operation"],"os_error":failure::os(error,receipt.map(|v|v.path.as_path())),"predecessor":self.state["predecessor"]})
        };
        let kind = if self.state["mode"] == "inventory" {
            "inventory-failure"
        } else {
            "migration-failure"
        };
        let value = transition::execute(&reg.backend, &reg.target, kind, &args)?;
        receipts::write(
            &reg.target
                .join(format!("resume-initialize-{}.failure.json", self.id)),
            &value,
        )
    }
}
