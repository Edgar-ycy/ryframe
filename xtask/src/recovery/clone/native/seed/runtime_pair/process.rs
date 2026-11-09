use super::{Pair, Role, StopKind, StopRecord, TIMEOUT, Value, controller, document, json};
use crate::{
    Result,
    recovery::{
        clone::native::{post_process::files, resources::common},
        full_stack::worker,
    },
};
use serde_json::Map;
use std::error::Error;

struct Started {
    role: Role,
    token: String,
    identity: Value,
}

struct StartFailure {
    error: Box<dyn Error>,
    untouched: bool,
}

impl StartFailure {
    fn untouched(error: Box<dyn Error>) -> Self {
        Self {
            error,
            untouched: true,
        }
    }

    fn retain(error: Box<dyn Error>) -> Self {
        Self {
            error,
            untouched: false,
        }
    }
}

impl Pair<'_> {
    pub(super) fn status(
        &self,
        roles: &[Role],
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<Value> {
        let mut processes = Map::new();
        for &role in roles {
            checkpoint()?;
            processes.insert(role.name().into(), self.observe(role, true)?);
        }
        checkpoint()?;
        Ok(Value::Object(processes))
    }

    pub(super) fn start(
        &self,
        roles: &[Role],
        checkpoint: &dyn Fn() -> Result<()>,
        record_start: &dyn Fn(Role, &str) -> Result<()>,
        record_stop: &StopRecord<'_>,
    ) -> Result<Value> {
        let _ = self.active_history()?;
        for &role in roles {
            self.prove_closed(role, checkpoint)?;
        }
        let mut started = Vec::new();
        for &role in roles {
            self.prove_closed(role, checkpoint)?;
            for previous in &started {
                self.require_running(previous, true, checkpoint)?;
            }
            match self.start_one(role, checkpoint, record_start) {
                Ok(value) => started.push(value),
                Err(failure) if failure.untouched && !started.is_empty() => {
                    let rollback = (|| {
                        // 下一角色尚未调用且没有任何待定控制，才能退回本次已成功的前一角色。
                        self.prove_closed(role, checkpoint)?;
                        let _ = self.active_history()?;
                        for previous in started.iter().rev() {
                            self.stop_one(
                                previous,
                                checkpoint,
                                StopKind::StartupRollback,
                                record_stop,
                            )?;
                        }
                        Ok(())
                    })();
                    return files::combine(Err(failure.error), rollback);
                }
                Err(failure) => return Err(failure.error),
            }
        }
        let mut processes = Map::new();
        for current in &started {
            self.require_running(current, true, checkpoint)?;
            processes.insert(
                current.role.name().into(),
                json!({"state":"running",
                "identity":current.identity,"ready":true,
                "log":format!("{}-{}.log", current.role.name(), current.token)}),
            );
        }
        Ok(Value::Object(processes))
    }

    fn start_one(
        &self,
        role: Role,
        checkpoint: &dyn Fn() -> Result<()>,
        record_start: &dyn Fn(Role, &str) -> Result<()>,
    ) -> std::result::Result<Started, StartFailure> {
        let inputs = &self.inputs;
        let reservation =
            worker::start::reserve(role, inputs.product, inputs.runtime, inputs.environment)
                .map_err(StartFailure::untouched)?;
        let token = reservation.id().to_owned();
        let producer = self
            .producer(role, &token)
            .map_err(StartFailure::untouched)?;
        producer.intent().map_err(StartFailure::retain)?;
        record_start(role, &token).map_err(StartFailure::retain)?;
        checkpoint().map_err(StartFailure::retain)?;
        // 启动 Err 可能晚到。缺少独立同代关闭证明时，保持原意图和已知前一角色。
        let result = reservation.start(TIMEOUT).map_err(StartFailure::retain)?;
        self.require_receipt(role, "start", &result, Some(&token))
            .map_err(StartFailure::retain)?;
        let identity =
            common::validate_process_identity(&result["identity"]).map_err(StartFailure::retain)?;
        producer.started(&identity).map_err(StartFailure::retain)?;
        let current = Started {
            role,
            token,
            identity,
        };
        self.require_running(&current, true, checkpoint)
            .map_err(StartFailure::retain)?;
        Ok(current)
    }

    pub(super) fn stop(
        &self,
        roles: &[Role],
        checkpoint: &dyn Fn() -> Result<()>,
        record_stop: &StopRecord<'_>,
    ) -> Result<Value> {
        if roles == [Role::Api, Role::Worker] {
            let pending = roles
                .iter()
                .copied()
                .filter_map(|role| {
                    let lock = self
                        .inputs
                        .runtime
                        .join(format!("{}-control.lock", role.name()));
                    match std::fs::symlink_metadata(&lock) {
                        Ok(_) => Some(
                            common::local_directory(self.inputs.backend, &lock, "原待定角色锁")
                                .map(|_| role),
                        ),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(error) => Some(Err(error.into())),
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            if let [unknown] = pending.as_slice() {
                let known = if *unknown == Role::Api {
                    Role::Worker
                } else {
                    Role::Api
                };
                self.stop_selected(&[known], checkpoint, record_stop)?;
                return Err(format!(
                    "{} 原控制仍待核对；{} 已按原证据收尾，禁止重放未知角色",
                    unknown.name(),
                    known.name()
                )
                .into());
            }
            if !pending.is_empty() {
                return Err("API 和 Worker 均有原待定操作，保留现场".into());
            }
        }
        self.stop_selected(roles, checkpoint, record_stop)
    }

    fn stop_selected(
        &self,
        roles: &[Role],
        checkpoint: &dyn Fn() -> Result<()>,
        record_stop: &StopRecord<'_>,
    ) -> Result<Value> {
        let history = self.active_history()?;
        let mut before = Vec::new();
        // 在任何停止调用前读完两侧完整前像。停止不请求或要求 HTTP ready。
        for &role in roles {
            checkpoint()?;
            let image = self.observe(role, false)?;
            let current = if image["state"] == "running" {
                let identity = common::validate_process_identity(&image["identity"])?;
                let token = active_token(history.as_ref(), role, &identity)?;
                Some(Started {
                    role,
                    token,
                    identity,
                })
            } else {
                None
            };
            before.push((role, current));
        }
        for (role, current) in before.iter().rev() {
            match current {
                Some(current) => {
                    self.stop_one(current, checkpoint, StopKind::Explicit, record_stop)?
                }
                None => self.prove_closed(*role, checkpoint)?,
            }
        }
        let mut processes = Map::new();
        for &role in roles {
            self.prove_closed(role, checkpoint)?;
            processes.insert(
                role.name().into(),
                json!({"state":"stopped","identity":null,"ready":false}),
            );
        }
        Ok(Value::Object(processes))
    }

    fn stop_one(
        &self,
        current: &Started,
        checkpoint: &dyn Fn() -> Result<()>,
        kind: StopKind,
        record_stop: &StopRecord<'_>,
    ) -> Result<()> {
        self.require_running(current, false, checkpoint)?;
        let producer = self.producer(current.role, &current.token)?;
        let inputs = &self.inputs;
        let result = controller::stop_bound_recorded(
            current.role,
            inputs.product,
            inputs.runtime,
            TIMEOUT,
            inputs.environment,
            controller::StopTarget {
                generation: controller::StopGeneration {
                    start_id: &current.token,
                    identity: &current.identity,
                },
                record: &|id| {
                    record_stop(kind, current.role, &current.token, &current.identity, id)
                },
                publish: &|| kind.publish(&producer, &current.identity, checkpoint),
            },
        )?;
        self.require_receipt(current.role, "stop", &result, None)?;
        self.prove_closed(current.role, checkpoint)
    }

    fn observe(&self, role: Role, ready: bool) -> Result<Value> {
        let inputs = &self.inputs;
        let observed = controller::observe_bound_state(
            role,
            inputs.product,
            inputs.runtime,
            inputs.environment,
            ready,
        )?;
        document::exact(
            &observed,
            &["state", "identity", "ready"],
            "seed 角色完整观察",
        )?;
        if !observed["ready"].is_boolean()
            || !matches!(observed["state"].as_str(), Some("running" | "stopped"))
            || (observed["state"] == "stopped"
                && (!observed["identity"].is_null() || observed["ready"] != false))
            || (!ready && observed["ready"] != false)
        {
            return Err("seed 角色完整观察状态无效".into());
        }
        Ok(observed)
    }

    fn require_running(
        &self,
        current: &Started,
        ready: bool,
        checkpoint: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        checkpoint()?;
        let observed = self.observe(current.role, ready)?;
        if observed["state"] != "running"
            || observed["identity"] != current.identity
            || (ready && observed["ready"] != true)
        {
            return Err("seed 角色不再属于本次确认的原运行身份".into());
        }
        checkpoint()
    }

    fn prove_closed(&self, role: Role, checkpoint: &dyn Fn() -> Result<()>) -> Result<()> {
        checkpoint()?;
        if self.observe(role, false)? != json!({"state":"stopped","identity":null,"ready":false}) {
            return Err("seed 角色没有当前完整关闭证明；保留原操作证据".into());
        }
        checkpoint()
    }

    fn require_receipt(
        &self,
        role: Role,
        operation: &str,
        receipt: &Value,
        reserved: Option<&str>,
    ) -> Result<()> {
        let state = if operation == "start" {
            "running"
        } else {
            "stopped"
        };
        if receipt["format_version"] != 1
            || receipt["kind"] != format!("full-stack-{}-control-result", role.name())
            || receipt["outcome"] != "succeeded"
            || receipt["state"] != state
            || receipt["operation"] != operation
            || receipt["scope_id"] != self.inputs.contract["scope_id"]
            || receipt["runtime_directory"] != json!(self.inputs.runtime)
            || (operation == "stop" && !receipt["identity"].is_null())
            || reserved.is_some_and(|id| {
                receipt["operation_id"] != id
                    || receipt["log"] != format!("{}-{id}.log", role.name())
            })
        {
            return Err("seed 角色原成功收据与固定合同或操作代次不同".into());
        }
        Ok(())
    }
}

pub(super) fn active_token(
    history: Option<&Value>,
    role: Role,
    identity: &Value,
) -> Result<String> {
    let history = history.ok_or("seed 运行角色缺少原启动历史")?;
    let event = document::array(&history["events"], "seed 角色历史")?
        .iter()
        .rfind(|event| event["role"] == role.name())
        .ok_or("seed 运行角色缺少原启动代次")?;
    if event["event"] != "started" || event["identity"] != *identity {
        return Err("seed 运行角色与原已启动代次不同；不停止、不重放".into());
    }
    Ok(document::text(&event["token"], "seed 角色启动操作 ID")?.to_owned())
}
