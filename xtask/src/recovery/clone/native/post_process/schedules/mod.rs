//! 每个调度操作都先持久记录意图，只有完整前像的显式核对才能允许另一次请求。

mod records;

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/post_process/schedules.rs"]
mod tests;

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::schedule_images::{action_input, inspect_disabled, validate_api_row, validate_before};
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common, target_verify::publication},
};

pub(super) struct Settings<'a> {
    pub(super) backend: &'a Path,
    pub(super) execution: &'a Path,
    pub(super) base: &'a Path,
    pub(super) output: &'a Path,
    pub(super) registration: &'a Value,
    pub(super) pending: &'a [Value],
    pub(super) administrator: &'a Value,
    pub(super) number: u64,
}

pub(super) trait ReadPort {
    fn checkpoint(&self) -> Result<()>;
    fn row(&mut self, action: &Value) -> Result<Value>;
}

pub(super) trait SessionPort: ReadPort {
    fn verify_administrator(&mut self) -> Result<()>;
    fn login(&mut self) -> Result<Value>;
    fn get(&mut self, action: &Value) -> Result<Value>;
    fn disable(&mut self, action: &Value) -> Result<Value>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Apply,
    Reconcile,
}

struct Action {
    value: Value,
    root: PathBuf,
    directories: Vec<PathBuf>,
}

pub(super) struct Prepared<'a> {
    settings: Settings<'a>,
    mode: Mode,
    actions: Vec<Action>,
}

impl<'a> Prepared<'a> {
    pub(super) fn load(settings: Settings<'a>, mode: Mode) -> Result<Self> {
        let actions = load_actions(&settings)?;
        if mode == Mode::Apply {
            for action in &actions {
                preflight(&settings, action)?;
            }
        }
        Ok(Self {
            settings,
            mode,
            actions,
        })
    }

    // 调用方在返回后执行有界注销和生产者回收，再发布阶段结果。
    pub(super) fn run(self, port: &mut impl SessionPort) -> Result<Value> {
        let settings = &self.settings;
        port.checkpoint()?;
        port.verify_administrator()?;
        let auth = port.login()?;
        authorization(settings.administrator, &auth)?;
        publication::publish(&settings.output.join("authentication.json"), &auth)?;
        let mut unresolved = Vec::new();
        for action in &self.actions {
            port.checkpoint()?;
            match self.mode {
                Mode::Apply => apply(settings, port, action)?,
                Mode::Reconcile => reconcile(settings, port, action, &mut unresolved)?,
            }
        }
        let (status, confirmed) = match self.mode {
            Mode::Apply => ("target_schedule_actions_verified", verify(settings, port)?),
            Mode::Reconcile if unresolved.is_empty() => {
                ("target_schedule_actions_reconciled", Vec::new())
            }
            Mode::Reconcile => ("needs_reconciliation", Vec::new()),
        };
        port.checkpoint()?;
        Ok(json!({
            "status":status,"confirmed":confirmed,"unresolved":unresolved,
            "registration":settings.registration,"worker_must_remain_stopped":true,
            "restore_qualified":false
        }))
    }
}

pub(super) fn verify(settings: &Settings<'_>, port: &mut impl ReadPort) -> Result<Vec<Value>> {
    let mut proofs = Vec::new();
    for action in load_actions(settings)? {
        let directory = action
            .directories
            .last()
            .ok_or("复制后调度动作尚未全部确认")?;
        let intent = records::Intent::load(settings, &action.value, directory)?;
        let proof = intent.confirmed()?;
        let observation = inspect(&intent, port, |_| Ok(None))?;
        proof.compare(&observation)?;
        proofs.push(proof.binding);
    }
    port.checkpoint()?;
    Ok(proofs)
}

fn load_actions(settings: &Settings<'_>) -> Result<Vec<Action>> {
    let mut seen = BTreeSet::new();
    let mut actions = Vec::new();
    common::local_directory(settings.backend, settings.base, "复制后处理根目录")?;
    for pending in settings.pending {
        let value = action_input(pending)?;
        let id = document::text(&value["schedule_id"], "调度 ID")?;
        if !seen.insert(id.to_owned()) {
            return Err("复制后调度计划包含重复 ID".into());
        }
        let root = settings.base.join(format!("schedule-{id}"));
        let directories = records::directories(settings.backend, &root)?;
        actions.push(Action {
            value,
            root,
            directories,
        });
    }
    Ok(actions)
}

fn authorization(admin: &Value, auth: &Value) -> Result<()> {
    let same = ["subject_id", "tenant_id", "username"]
        .iter()
        .all(|key| admin.get(key).is_some() && auth.get(key) == admin.get(key));
    let permissions = auth["permissions"].as_array();
    let has = |permission: &str| {
        permissions.is_some_and(|items| items.iter().any(|item| item.as_str() == Some(permission)))
    };
    if same
        && (auth["is_super_admin"].as_bool() == Some(true)
            || has("*")
            || (has("monitor:schedule:list") && has("monitor:schedule:edit")))
    {
        Ok(())
    } else {
        Err("实际目标会话与登记管理员或调度权限不匹配".into())
    }
}

fn preflight(settings: &Settings<'_>, action: &Action) -> Result<()> {
    let Some(directory) = action.directories.last() else {
        return Ok(());
    };
    if records::present(settings.backend, &directory.join("confirmed.json"))? {
        return records::Intent::load(settings, &action.value, directory)?
            .confirmed()
            .map(|_| ());
    }
    if records::present(settings.backend, &directory.join("intent.json"))? {
        return records::Intent::load(settings, &action.value, directory)?.allow_new_attempt();
    }
    records::check_preparation(settings.backend, directory)
}

fn apply(settings: &Settings<'_>, port: &mut impl SessionPort, action: &Action) -> Result<()> {
    preflight(settings, action)?;
    if let Some(directory) = action.directories.last()
        && records::present(settings.backend, &directory.join("confirmed.json"))?
    {
        let intent = records::Intent::load(settings, &action.value, directory)?;
        let proof = intent.confirmed()?;
        return proof.compare(&inspect(&intent, port, |port| {
            port.get(&action.value).map(Some)
        })?);
    }
    records::create_root(settings.backend, &action.root)?;
    let directory = action
        .root
        .join(format!("{:04}", action.directories.len() + 1));
    fs::create_dir(&directory)?;
    common::local_directory(settings.backend, &directory, "新调度操作目录")?;
    let action = &action.value;
    port.checkpoint()?;
    let before = port.row(action)?;
    validate_before(settings.execution, action, &before)?;
    let api_before = port.get(action)?;
    validate_api_row(&api_before, action, true, version(action, false)?)?;
    let intent =
        records::Intent::prepare(settings, action, &directory, &before, &api_before, port)?;
    let response = port.disable(action)?;
    publication::publish(&directory.join("api-response.json"), &response)?;
    validate_api_row(&response, action, false, version(action, true)?)?;
    let observed = inspect(&intent, port, |port| port.get(action).map(Some))?;
    publication::publish(&directory.join("after.json"), &observed)?;
    port.checkpoint()?;
    intent.save_confirmation(&observed, Some("api-response.json"), "after.json")
}

fn reconcile(
    settings: &Settings<'_>,
    port: &mut impl SessionPort,
    action: &Action,
    unresolved: &mut Vec<Value>,
) -> Result<()> {
    let Some(directory) = action.directories.last() else {
        return Ok(());
    };
    if !records::present(settings.backend, &directory.join("intent.json"))? {
        return records::check_preparation(settings.backend, directory);
    }
    let action = &action.value;
    let intent = records::Intent::load(settings, action, directory)?;
    let observed = inspect(&intent, port, |port| port.get(action).map(Some))?;
    if records::present(settings.backend, &directory.join("confirmed.json"))? {
        return intent.confirmed()?.compare(&observed);
    }
    let name = format!("reconcile-{:04}.json", settings.number);
    intent.save_reconciliation(&name, &observed)?;
    port.checkpoint()?;
    match observed["inspection"]["status"].as_str() {
        Some("disabled_row_verified") => intent.save_confirmation(&observed, None, &name),
        Some("unchanged_before") => Ok(()),
        Some("needs_reconciliation") => {
            unresolved.push(action["schedule_id"].clone());
            Ok(())
        }
        _ => Err("无法识别调度核对结果".into()),
    }
}

fn inspect<P: ReadPort>(
    intent: &records::Intent<'_, '_>,
    port: &mut P,
    read_api: impl FnOnce(&mut P) -> Result<Option<Value>>,
) -> Result<Value> {
    intent.check()?;
    port.checkpoint()?;
    let current = port.row(intent.action)?;
    let inspection = inspect_disabled(
        intent.settings.execution,
        intent.action,
        &intent.before,
        &current,
    )?;
    let api = read_api(port)?;
    if let Some(api) = api.as_ref()
        && inspection["status"] != "needs_reconciliation"
    {
        let disabled = inspection["status"] == "disabled_row_verified";
        validate_api_row(
            api,
            intent.action,
            !disabled,
            version(intent.action, disabled)?,
        )?;
    }
    intent.check()?;
    port.checkpoint()?;
    Ok(json!({"inspection":inspection,"current":current,"api":api}))
}

fn version(action: &Value, increment: bool) -> Result<i64> {
    action["expected_version"]
        .as_i64()
        .and_then(|version| version.checked_add(i64::from(increment)))
        .ok_or_else(|| "调度版本无效".into())
}
