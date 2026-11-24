//! 复制账本的受管会话。资源写入与完整前后像由正式复制调用方提供。
use std::{collections::BTreeMap, fs, io::Write, path::Path};

use serde_json::{Value, json};
use time::OffsetDateTime;

use super::{
    common, encoded, inspect, require_complete,
    storage::{Seal, Storage},
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{
        copy_owner::{Owner, Role},
        lifecycle::lock::ProcessGuard,
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::recovery::clone::native) enum Mode {
    New,
    Reconcile,
    Resume,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::New => "apply",
            Self::Reconcile => "reconcile",
            Self::Resume => "resume",
        }
    }
}

pub(in crate::recovery::clone::native) struct Session<'a> {
    storage: Storage<'a>,
    finishing: bool,
    writes: BTreeMap<String, bool>,
}

impl<'a> Session<'a> {
    fn open(
        window: &'a Window<'_>,
        mode: Mode,
        verify_owner: &'a dyn Fn(&Path) -> Result<()>,
    ) -> Result<Self> {
        let (backend, plan, generation) = window.owner.ledger_binding();
        let root = window.root;
        let mut storage = match mode {
            Mode::New => Storage::create(backend, root, plan, generation, verify_owner)?,
            _ => Storage::open(backend, root, plan, generation, verify_owner)?,
        };
        if mode == Mode::Resume
            && storage
                .projection()?
                .steps
                .values()
                .any(|step| matches!(step["phase"].as_str(), Some("unknown" | "mismatch")))
        {
            return Err("未知结果必须先显式核对，不能继续写入".into());
        }
        storage.append(json!({
            "type":"opened", "session":window.owner.session_id(), "mode":mode.name()
        }))?;
        Ok(Self {
            storage,
            finishing: false,
            writes: BTreeMap::new(),
        })
    }

    pub(in crate::recovery::clone::native) fn steps(
        &self,
    ) -> Result<serde_json::Map<String, Value>> {
        Ok(self.storage.projection()?.steps)
    }

    /// 调用方先持久化 intent 或 resumed。当前会话内每步只发起一次资源写入。
    pub(in crate::recovery::clone::native) fn write_once<T>(
        &mut self,
        step: &str,
        before: &Value,
        after: &Value,
        artifact: &str,
        execute: impl FnOnce(&dyn Fn() -> Result<()>) -> Result<T>,
    ) -> Result<T> {
        self.check_pending_write(step, before, after, artifact)?;
        if self.writes.contains_key(step) {
            return Err("本会话已尝试此资源写入，必须保留现场并显式核对".into());
        }
        self.writes.insert(step.to_owned(), false);
        let result = execute(&|| self.check_pending_write(step, before, after, artifact));
        if result.is_ok() {
            self.writes.insert(step.to_owned(), true);
        }
        result
    }

    fn check_pending_write(
        &self,
        step: &str,
        before: &Value,
        after: &Value,
        artifact: &str,
    ) -> Result<()> {
        self.require_open()?;
        let projection = self.storage.projection()?;
        let session = projection
            .session
            .as_ref()
            .filter(|session| session.outcome == "open" && session.mode != "reconcile")
            .ok_or("资源写入必须属于当前持锁的 apply 或 resume 会话")?;
        let expected = projection.steps.get(step).ok_or("资源写入缺少原 intent")?;
        if expected["phase"] != "unknown"
            || expected["session"] != session.id
            || &expected["before"] != before
            || &expected["after"] != after
            || expected["artifact_sha256"].as_str() != Some(artifact)
        {
            return Err("资源写入与本会话的原意图或续跑前像不同".into());
        }
        Ok(())
    }

    pub(in crate::recovery::clone::native) fn intent(
        &mut self,
        step: &str,
        before: &Value,
        after: &Value,
        artifact: &str,
    ) -> Result<()> {
        self.event(json!({
            "type":"intent", "step_id":step, "before":before, "after":after, "artifact_sha256":artifact
        }))
    }

    pub(in crate::recovery::clone::native) fn confirm(
        &mut self,
        step: &str,
        after: &Value,
    ) -> Result<()> {
        if self.writes.get(step) != Some(&true) {
            return Err("资源写入未通过本会话的执行核对，必须显式核对后像".into());
        }
        self.event(json!({"type":"confirmed","step_id":step,"observed":after}))
    }

    pub(in crate::recovery::clone::native) fn reconcile(
        &mut self,
        step: &str,
        observed: &Value,
    ) -> Result<String> {
        self.event(json!({"type":"reconciled","step_id":step,"observed":observed}))?;
        Ok(super::document::text(&self.steps()?[step]["phase"], "核对后步骤状态")?.to_owned())
    }

    pub(in crate::recovery::clone::native) fn resume(
        &mut self,
        step: &str,
        before: &Value,
    ) -> Result<()> {
        self.event(json!({"type":"resumed","step_id":step,"observed":before}))
    }

    pub(in crate::recovery::clone::native) fn finish(&mut self) -> Result<()> {
        self.require_open()?;
        require_complete(&self.storage.projection()?)?;
        self.finishing = true;
        Ok(())
    }

    fn event(&mut self, event: Value) -> Result<()> {
        self.require_open()?;
        self.storage.append(event)
    }

    fn require_open(&self) -> Result<()> {
        if self.finishing {
            return Err("复制账本已开始收尾".into());
        }
        Ok(())
    }

    fn close(mut self, passed: bool) -> Result<Seal> {
        if self.storage.poisoned() {
            return Err("账本写入结果未知，无法关闭会话或发布完成收据".into());
        }
        let (outcome, completed_at) = match (passed, self.finishing) {
            (true, true) => ("publishing", json!(completed_at())),
            (true, false) => ("partial", Value::Null),
            (false, _) => ("failed", Value::Null),
        };
        self.storage
            .append(json!({"type":"closed","outcome":outcome,"completed_at":completed_at}))?;
        self.storage.seal()
    }
}

struct Window<'a> {
    owner: &'a Owner,
    root: &'a Path,
    target_guard: &'a ProcessGuard,
    target_lock: &'a Value,
    ledger_guard: ProcessGuard,
}

impl<'a> Window<'a> {
    fn acquire(
        owner: &'a Owner,
        target_guard: &'a ProcessGuard,
        target_lock: &'a Value,
        mode: Mode,
    ) -> Result<Self> {
        owner.check_live_lock(Role::Target, target_guard, target_lock)?;
        let root = owner
            .lock_path(Role::Ledger)
            .parent()
            .ok_or("账本锁缺少账本目录")?;
        let (backend, _, _) = owner.ledger_binding();
        if mode == Mode::New {
            validate_local_test_path(root, backend, LocalTestPathKind::NewDirectory)?;
            fs::create_dir(root)?;
        }
        common::local_directory(backend, root, "复制账本目录")?;
        let ledger_guard = ProcessGuard::acquire_named(root, Role::Ledger.guard_name())?;
        let window = Self {
            owner,
            root,
            target_guard,
            target_lock,
            ledger_guard,
        };
        window.check()?;
        Ok(window)
    }

    fn check(&self) -> Result<()> {
        self.owner
            .check_live_lock(Role::Target, self.target_guard, self.target_lock)?;
        self.ledger_guard
            .check_named(self.root, Role::Ledger.guard_name())
    }

    fn check_ledger(&self, root: &Path, record: &Value) -> Result<()> {
        if root != self.root {
            return Err("复制账本不属于同一控制窗口".into());
        }
        self.check()?;
        self.owner
            .check_live_lock(Role::Ledger, &self.ledger_guard, record)
    }

    fn check_unlocked(&self, seal: &Seal) -> Result<()> {
        self.check()?;
        if self.owner.verify_lock(Role::Ledger)?.is_some() {
            return Err("账本锁仍在，不能发布完成收据".into());
        }
        seal.check(self.owner.ledger_binding().0)?;
        self.check()
    }

    fn fault(&self, record: &Value, seal: &Seal, stage: &str) -> Result<()> {
        let (backend, plan, generation) = self.owner.ledger_binding();
        seal.check(backend)?;
        let verify = |root: &Path| self.check_ledger(root, record);
        let mut storage = Storage::open(backend, self.root, plan, generation, &verify)?;
        storage.append(json!({"type":"fault","stage":stage}))
    }

    fn receipt_fault(&self, seal: &Seal) -> Result<()> {
        self.check_unlocked(seal)?;
        let record = self.owner.claim_lock(Role::Ledger, &self.ledger_guard)?;
        let fault = self.fault(&record, seal, "receipt_write");
        let release = self
            .owner
            .release_lock(Role::Ledger, &self.ledger_guard, &record);
        combine(fault, release)
    }
}

pub(in crate::recovery::clone::native) fn run<T>(
    owner: &Owner,
    target_guard: &ProcessGuard,
    target_lock: &Value,
    mode: Mode,
    execute: impl FnOnce(&mut Session<'_>) -> Result<T>,
) -> Result<(T, Value)> {
    let window = Window::acquire(owner, target_guard, target_lock, mode)?;
    let record = owner.claim_lock(Role::Ledger, &window.ledger_guard)?;
    let verify = |root: &Path| window.check_ledger(root, &record);
    let (result, seal) = execute_and_close(Session::open(&window, mode, &verify), execute);
    if let Err(release) = owner.release_lock(Role::Ledger, &window.ledger_guard, &record) {
        let fault = seal
            .as_ref()
            .map_or(Ok(()), |seal| window.fault(&record, seal, "lock_release"));
        let error = match result {
            Ok(_) => release,
            Err(error) => format!("{error}；账本锁释放失败：{release}").into(),
        };
        return combine(Err(error), fault);
    }
    let value = result?;
    let seal = seal.ok_or("复制账本缺少已同步关闭末帧")?;
    let (_, plan, generation) = owner.ledger_binding();
    let receipt = seal.completion(plan, generation)?;
    let verified = publish_and_inspect(&window, &seal, receipt.as_ref());
    match verified {
        Err(error) if receipt.is_some() => combine(Err(error), window.receipt_fault(&seal)),
        result => result.map(|verified| (value, verified)),
    }
}

fn execute_and_close<T>(
    session: Result<Session<'_>>,
    execute: impl FnOnce(&mut Session<'_>) -> Result<T>,
) -> (Result<T>, Option<Seal>) {
    let mut session = match session {
        Ok(session) => session,
        Err(error) => return (Err(error), None),
    };
    let result = execute(&mut session);
    match session.close(result.is_ok()) {
        Ok(seal) => (result, Some(seal)),
        Err(error) => (combine(result, Err(error)), None),
    }
}

fn publish_and_inspect(window: &Window<'_>, seal: &Seal, receipt: Option<&Value>) -> Result<Value> {
    window.check_unlocked(seal)?;
    let (backend, plan, generation) = window.owner.ledger_binding();
    if let Some(receipt) = receipt {
        let path = window
            .root
            .join(format!("complete-{}.json", window.owner.session_id()));
        let mut file = tempfile::Builder::new()
            .prefix(".complete-")
            .suffix(".tmp")
            .disable_cleanup(true)
            .tempfile_in(window.root)?;
        file.write_all(&encoded(receipt)?)?;
        file.as_file().sync_all()?;
        window.check_unlocked(seal)?;
        file.persist_noclobber(path).map_err(|error| error.error)?;
    }
    window.check_unlocked(seal)?;
    let evidence = inspect(backend, window.root, plan, generation)?;
    if let Some(receipt) = receipt
        && (evidence["status"] != "ledger_evidence_complete"
            || evidence["completed_at"] != receipt["completed_at"])
    {
        return Err("复制账本完成收据未通过独立只读核验".into());
    }
    window.check_unlocked(seal)?;
    Ok(evidence)
}

fn completed_at() -> String {
    crate::utc_timestamp::rfc3339_seconds_or_micros(OffsetDateTime::now_utc())
}

fn combine<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(other)) => Err(format!("{error}；账本收尾失败：{other}").into()),
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_ledger/session.rs"]
pub(in crate::recovery::clone::native) mod tests;
