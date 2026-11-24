//! 单次续作保存原始失败、确认进度与安全OS属性；异常路径不推断命令是否执行。
use super::super::super::super::super::{export::transport::Transport, resources::common};
use super::super::super::super::resume::transition;
use super::super::super::{failure, receipts, registration::Registration};
use super::super::window::Window;
use crate::Result;
use crate::utc_timestamp::now_rfc3339;
use serde_json::{Value, json};
use std::path::PathBuf;

pub(crate) struct Attempt {
    pub(in super::super) id: String,
    pub(in super::super) state: Value,
    pub(in super::super) session: Value,
    pub(in super::super) intent: PathBuf,
    pub(in super::super) started: PathBuf,
    intent_binding: Option<Value>,
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
            intent_binding: None,
        })
    }
    pub(in super::super) fn now(&self) -> String {
        now_rfc3339()
    }
    pub(in super::super) fn transition<T: Transport>(
        &self,
        w: &Window<'_, T>,
        kind: &str,
        args: &Value,
    ) -> Result<Value> {
        transition::execute(w.inputs.backend, w.inputs.observation_dir, kind, args)
    }
    pub(in super::super) fn publish_intent(&mut self, value: &Value) -> Result<()> {
        receipts::write(&self.intent, value)?;
        self.intent_binding = Some(common::binding(&self.intent)?);
        Ok(())
    }
    pub(in super::super) fn command_paths<T: Transport>(
        &self,
        w: &Window<'_, T>,
        label: &str,
        before: &Value,
    ) -> Result<Vec<PathBuf>> {
        use super::super::super::super::{resume_observations, resume_prefix, tree};
        let intent = common::read_bound(
            w.inputs.backend,
            self.intent_binding
                .as_ref()
                .ok_or("续作命令缺少已发布intent绑定")?,
            "续作命令intent",
        )?;
        if self.state["mode"] != "inventory" && intent["files_before"] != self.state["files"] {
            return Err("迁移intent完整前像不同于本次已核验状态".into());
        }
        let old = resume_observations::file_map(before)?;
        let current = resume_observations::file_map(&tree::snapshot(
            w.inputs.backend,
            w.inputs.observation_dir,
            true,
        )?)?;
        if old
            .iter()
            .any(|(name, value)| current.get(name) != Some(value))
        {
            return Err("续作命令期间历史文件缺失或改写，禁止吸收新收据".into());
        }
        let historical = if self.state["mode"] == "inventory" {
            std::collections::BTreeSet::new()
        } else {
            resume_prefix::prior_failed_stage(
                w.inputs.backend,
                w.inputs.observation_dir,
                &intent["files_before"],
                label,
            )?
        };
        let names = current.keys().cloned().collect();
        let mut paths = Vec::new();
        for name in resume_observations::stage(&names, label)? {
            let path = w.inputs.observation_dir.join(&name);
            if old.contains_key(&name) {
                common::bound_file(w.inputs.backend, &self.state["failure"], "原始失败")?;
                if !historical.contains(&name) {
                    return Err("历史同阶段收据没有严格原失败绑定".into());
                }
            } else {
                paths.push(path);
            }
        }
        let claimed = paths
            .iter()
            .filter_map(|path| path.file_name().and_then(|v| v.to_str()))
            .collect::<std::collections::BTreeSet<_>>();
        if current
            .keys()
            .filter(|name| !old.contains_key(*name))
            .any(|name| {
                !claimed.contains(name.as_str())
                    && !(self.state["mode"] == "inventory" && *name == format!("{label}.json"))
            })
        {
            return Err("命令观察出现未绑定的新增文件或目录，保留现场并拒绝续作".into());
        }
        if paths.len() > 1 {
            return Err("同一操作产生重复命令收据，保留现场并拒绝续作".into());
        }
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
