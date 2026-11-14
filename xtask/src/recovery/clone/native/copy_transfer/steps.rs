//! 按完整计划编排前后像、一次性写入及原复制账本。
use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::{Result, recovery::source::evidence::reference::copy_step_ids};

use super::super::{
    copy_ledger::{
        model,
        session::{Mode, Session},
    },
    input::digest_shape,
    json as document,
};

const OBJECT_WORKERS: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::recovery::clone::native) enum Kind {
    Database,
    Object,
}

pub(in crate::recovery::clone::native) struct Step<'a> {
    pub(in crate::recovery::clone::native) id: String,
    pub(in crate::recovery::clone::native) item: &'a Value,
    pub(in crate::recovery::clone::native) kind: Kind,
    pub(in crate::recovery::clone::native) artifact: &'a str,
}

#[derive(PartialEq, Eq)]
pub(in crate::recovery::clone::native) struct Basis {
    pub(in crate::recovery::clone::native) before: Value,
    pub(in crate::recovery::clone::native) after: Value,
}

pub(in crate::recovery::clone::native) struct Plan<'a> {
    databases: Vec<Step<'a>>,
    objects: Vec<Step<'a>>,
}

impl<'a> Plan<'a> {
    pub(in crate::recovery::clone::native) fn from_verified(plan: &'a Value) -> Result<Self> {
        let databases = document::array(&plan["databases"], "复制数据库计划")?;
        let objects = document::array(&plan["objects"], "复制对象计划")?;
        let ids = copy_step_ids(&json!({
            "databases": databases.iter().map(|item| json!({"key": item["key"]})).collect::<Vec<_>>(),
            "objects": objects.iter().map(|item| json!({
                "bucket": item["bucket"], "target_key": item["target_key"],
            })).collect::<Vec<_>>(),
        }))?;
        Ok(Self {
            databases: build_steps(databases, &ids["databases"], Kind::Database)?,
            objects: build_steps(objects, &ids["objects"], Kind::Object)?,
        })
    }

    pub(super) fn all(&self) -> impl Iterator<Item = &Step<'a>> {
        self.databases.iter().chain(&self.objects)
    }

    pub(super) fn validate_states(&self, states: &Map<String, Value>, mode: Mode) -> Result<()> {
        let planned: BTreeSet<_> = self.all().map(|step| step.id.as_str()).collect();
        if states.keys().any(|id| !planned.contains(id.as_str()))
            || (mode == Mode::New && !states.is_empty())
        {
            return Err("账本步骤与完整复制计划或新建模式不同".into());
        }
        if mode == Mode::Resume
            && states.values().any(|item| {
                !matches!(
                    item["phase"].as_str(),
                    Some("confirmed" | "reconciled_before")
                )
            })
        {
            return Err("未知或不匹配步骤必须先显式核对".into());
        }
        Ok(())
    }
}

fn build_steps<'a>(items: &'a [Value], ids: &Value, kind: Kind) -> Result<Vec<Step<'a>>> {
    let ids = document::array(ids, "复制步骤编号")?;
    if ids.len() != items.len() {
        return Err("复制步骤编号未覆盖完整计划".into());
    }
    items
        .iter()
        .zip(ids)
        .map(|(item, id)| {
            Ok(Step {
                id: document::text(id, "复制步骤编号")?.to_owned(),
                item,
                kind,
                artifact: digest_shape(&item["artifact"]["sha256"])?,
            })
        })
        .collect()
}

pub(in crate::recovery::clone::native) trait CopyIo {
    type Evidence;

    /// 检查冻结计划、工具、双侧代次与停止证明。单对象步骤同时复查原对象。
    fn checkpoint(&mut self, selected: Option<&Step<'_>>) -> Result<()>;
    /// 续跑前核对目标 Redis 已登记代次及原初始化后像。
    fn check_resume_target(&mut self) -> Result<()>;
    /// 校验原始 artifact 及本步骤绑定的证据文件，再生成同代次前后像。
    fn basis(&mut self, step: &Step<'_>) -> Result<Basis>;
    /// 每次观察完整的源和目标数据库向量，按计划步骤编号返回目标完整像。
    fn observe_databases(&mut self, all: &[Step<'_>]) -> Result<Observation<Self::Evidence>>;
    /// 独立观察选定的目标对象，复核收据与来源绑定后按步骤编号返回完整像。
    fn observe_objects(&mut self, selected: &[&Step<'_>]) -> Result<Observation<Self::Evidence>>;
    /// 用同一批采样的收据和原始响应复核投影；不重复发出远端请求。
    fn check_observation(&mut self, observation: &Observation<Self::Evidence>) -> Result<()>;
    /// 在同一持锁会话的 write_once 内执行受限 SQL 或对象条件创建。
    fn write(&mut self, step: &Step<'_>, session: &mut Session<'_>, basis: &Basis) -> Result<()>;
    /// 对全量双侧库、对象索引和 owner、目标 Redis 及停止证明做收尾观察。
    fn verify_complete(&mut self) -> Result<()>;
}

pub(in crate::recovery::clone::native) struct Observation<E> {
    pub(in crate::recovery::clone::native) images: Map<String, Value>,
    pub(in crate::recovery::clone::native) evidence: E,
}

pub(in crate::recovery::clone::native) enum Outcome {
    Verified {
        unchanged_databases: Map<String, Value>,
    },
    Reconciled(Value),
}

pub(in crate::recovery::clone::native) fn run(
    plan: &Plan<'_>,
    session: &mut Session<'_>,
    mode: Mode,
    io: &mut impl CopyIo,
) -> Result<Outcome> {
    io.checkpoint(None)?;
    let initial = session.steps()?;
    plan.validate_states(&initial, mode)?;
    let mut noops = Map::new();
    if mode == Mode::Reconcile {
        let steps = check_batches(plan, session, io, Check::Reconcile, &noops)?;
        let pending = plan
            .all()
            .filter(|step| !initial.contains_key(&step.id))
            .map(|step| step.id.as_str())
            .collect::<BTreeSet<_>>();
        let status = if steps.values().any(|phase| phase == "mismatch") {
            "needs_reconciliation"
        } else {
            "copy_reconciled"
        };
        return Ok(Outcome::Reconciled(json!({
            "status": status, "steps": steps, "pending_steps": pending, "remote_writes": 0,
            "target_ready": false, "restore_success": false,
        })));
    }
    if mode == Mode::Resume {
        io.check_resume_target()?;
        check_batches(plan, session, io, Check::Confirmed, &noops)?;
    }
    for step in plan.all() {
        match initial
            .get(&step.id)
            .and_then(|state| state["phase"].as_str())
        {
            Some("confirmed") => {}
            Some("reconciled_before") => apply(plan, step, session, io, true, &mut noops)?,
            None => apply(plan, step, session, io, false, &mut noops)?,
            _ => return Err("复制步骤缺少可执行状态".into()),
        }
    }
    finish(plan, session, io, &noops)?;
    Ok(Outcome::Verified {
        unchanged_databases: noops,
    })
}

fn apply(
    plan: &Plan<'_>,
    step: &Step<'_>,
    session: &mut Session<'_>,
    io: &mut impl CopyIo,
    resume: bool,
    noops: &mut Map<String, Value>,
) -> Result<()> {
    io.checkpoint(Some(step))?;
    let states = session.steps()?;
    let basis = bound_basis(io, step)?;
    let existing = states.get(&step.id);
    check_intent(step, &basis, existing)?;
    if resume != existing.is_some() {
        return Err("当前步骤存在未获授权的原意图或缺少续跑意图".into());
    }
    let observed = observe_step(plan, step, io)?;
    check_window(io, Some(step), &[(step, &basis)], session, &states)?;
    io.check_observation(&observed)?;
    let before = &observed.images[&step.id];
    if before != &basis.before {
        return Err("当前资源偏离同代次完整初始前像，禁止接管或重放".into());
    }
    if basis.before == basis.after {
        noops.insert(step.id.clone(), basis.before);
        return Ok(());
    }
    if resume {
        session.resume(&step.id, before)?;
    } else {
        session.intent(&step.id, &basis.before, &basis.after, step.artifact)?;
    }
    let pending = session.steps()?;
    check_window(io, Some(step), &[(step, &basis)], session, &pending)?;
    io.write(step, session, &basis)?;
    let after = observe_step(plan, step, io)?;
    check_window(io, Some(step), &[(step, &basis)], session, &pending)?;
    io.check_observation(&after)?;
    session.confirm(&step.id, &after.images[&step.id])
}

fn bound_basis(io: &mut impl CopyIo, step: &Step<'_>) -> Result<Basis> {
    let basis = io.basis(step)?;
    let kind = match step.kind {
        Kind::Database => "database",
        Kind::Object => "object",
    };
    if basis.before["kind"] != kind || basis.after["kind"] != kind {
        return Err("步骤前后像的资源类别与计划不同".into());
    }
    if step.kind == Kind::Database && basis.before == basis.after {
        model::image(&basis.before)?;
    } else {
        model::image_pair(&basis.before, &basis.after)?;
    }
    Ok(basis)
}

fn check_intent(step: &Step<'_>, basis: &Basis, state: Option<&Value>) -> Result<()> {
    if state.is_some_and(|state| {
        state["before"] != basis.before
            || state["after"] != basis.after
            || state["artifact_sha256"] != step.artifact
            || basis.before == basis.after
    }) {
        return Err("原账本意图与同代次完整计划或原始 artifact 不同".into());
    }
    Ok(())
}

fn observe_step<I: CopyIo>(
    plan: &Plan<'_>,
    step: &Step<'_>,
    io: &mut I,
) -> Result<Observation<I::Evidence>> {
    match step.kind {
        Kind::Database => database_vector(plan, io),
        Kind::Object => object_vector(&[step], io),
    }
}

fn database_vector<I: CopyIo>(plan: &Plan<'_>, io: &mut I) -> Result<Observation<I::Evidence>> {
    let observed = io.observe_databases(&plan.databases)?;
    validate_vector(plan.databases.iter(), &observed.images)?;
    Ok(observed)
}

fn object_vector<I: CopyIo>(
    selected: &[&Step<'_>],
    io: &mut I,
) -> Result<Observation<I::Evidence>> {
    let observed = io.observe_objects(selected)?;
    validate_vector(selected.iter().copied(), &observed.images)?;
    Ok(observed)
}

fn validate_vector<'a>(
    selected: impl ExactSizeIterator<Item = &'a Step<'a>>,
    observed: &Map<String, Value>,
) -> Result<()> {
    if selected.len() != observed.len() {
        return Err("完整资源观察缺少或包含计划外步骤".into());
    }
    for step in selected {
        model::image(observed.get(&step.id).ok_or("完整资源观察缺少计划步骤")?)?;
    }
    Ok(())
}

fn check_window(
    io: &mut impl CopyIo,
    selected: Option<&Step<'_>>,
    expected: &[(&Step<'_>, &Basis)],
    session: &Session<'_>,
    states: &Map<String, Value>,
) -> Result<()> {
    io.checkpoint(selected)?;
    for (step, basis) in expected {
        if &bound_basis(io, step)? != *basis {
            return Err("观察窗口内冻结步骤前后像变化".into());
        }
    }
    if &session.steps()? != states {
        return Err("观察窗口内复制账本变化".into());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
    Reconcile,
    Confirmed,
    Finish,
}

fn check_batches(
    plan: &Plan<'_>,
    session: &mut Session<'_>,
    io: &mut impl CopyIo,
    check: Check,
    noops: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let states = session.steps()?;
    let selected = |step: &&Step<'_>| {
        check != Check::Confirmed
            || states
                .get(&step.id)
                .is_some_and(|state| state["phase"] == "confirmed")
    };
    let databases: Vec<_> = plan.databases.iter().filter(selected).collect();
    let objects: Vec<_> = plan.objects.iter().filter(selected).collect();
    let mut result = Map::new();
    if !databases.is_empty() {
        let batch = Batch {
            selected: &databases,
            check,
            noops,
        };
        result.extend(batch.check(session, io, |io| database_vector(plan, io))?);
    }
    for selected in objects.chunks(OBJECT_WORKERS) {
        let batch = Batch {
            selected,
            check,
            noops,
        };
        result.extend(batch.check(session, io, |io| object_vector(selected, io))?);
    }
    Ok(result)
}

struct Batch<'a> {
    selected: &'a [&'a Step<'a>],
    check: Check,
    noops: &'a Map<String, Value>,
}

impl Batch<'_> {
    fn check<I: CopyIo>(
        &self,
        session: &mut Session<'_>,
        io: &mut I,
        observe: impl FnOnce(&mut I) -> Result<Observation<I::Evidence>>,
    ) -> Result<Map<String, Value>> {
        io.checkpoint(None)?;
        let states = session.steps()?;
        let bases = self
            .selected
            .iter()
            .map(|step| {
                let basis = bound_basis(io, step)?;
                check_intent(step, &basis, states.get(&step.id))?;
                Ok(basis)
            })
            .collect::<Result<Vec<_>>>()?;
        let expected: Vec<_> = self.selected.iter().copied().zip(&bases).collect();
        let observed = observe(io)?;
        check_window(io, None, &expected, session, &states)?;
        io.check_observation(&observed)?;
        let images = observed.images;
        for (step, basis) in &expected {
            self.verify_image(step, basis, states.get(&step.id), &images[&step.id])?;
        }
        let mut result = Map::new();
        if self.check == Check::Reconcile {
            for step in self
                .selected
                .iter()
                .filter(|step| states.contains_key(&step.id))
            {
                let phase = session.reconcile(&step.id, &images[&step.id])?;
                result.insert(step.id.clone(), json!(phase));
            }
        }
        Ok(result)
    }

    fn verify_image(
        &self,
        step: &Step<'_>,
        basis: &Basis,
        state: Option<&Value>,
        image: &Value,
    ) -> Result<()> {
        if image["kind"] != basis.before["kind"]
            || image["resource_sha256"] != basis.before["resource_sha256"]
        {
            return Err("独立观察的物理资源身份与同代次计划不同".into());
        }
        if self.check == Check::Reconcile {
            if state.is_none() && image != &basis.before {
                return Err("未登记写入的资源已偏离初始前像，禁止接管".into());
            }
            return Ok(());
        }
        let covered = match state {
            Some(state) => state["phase"] == "confirmed",
            None => {
                self.check == Check::Finish
                    && step.kind == Kind::Database
                    && self.noops.get(&step.id) == Some(&basis.before)
                    && basis.before == basis.after
            }
        };
        if !covered || image != &basis.after {
            return Err("完整后像未确认或当前观察不同，必须停止并显式核对".into());
        }
        Ok(())
    }
}

fn finish(
    plan: &Plan<'_>,
    session: &mut Session<'_>,
    io: &mut impl CopyIo,
    noops: &Map<String, Value>,
) -> Result<()> {
    let states = session.steps()?;
    let planned = plan.all().count();
    if states.len() + noops.len() != planned || noops.keys().any(|id| states.contains_key(id)) {
        return Err("完整计划仍有未确认或未只读核验的数据步骤".into());
    }
    check_batches(plan, session, io, Check::Finish, noops)?;
    io.verify_complete()?;
    check_window(io, None, &[], session, &states)?;
    if !states.is_empty() {
        session.finish()?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/steps.rs"]
mod tests;
