//! 用原停止操作编号及启动身份补录中断的停止历史。
use super::super::super::runtime_pair::StopKind;
use super::{
    Frame, PAIR, Producer, Role, claim_path, common, controller, directory, document, hex_id,
    history, reservation, reserved_path, state, verify_claim,
};
use crate::{
    Result,
    recovery::clone::native::{post_process::files, target_verify::guard::Inputs},
};
use serde_json::{Value, json};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

#[path = "archived_stop/projection.rs"]
mod projection;
pub(in crate::recovery::clone::native::seed::pair_entry) use projection::{
    controller_binding as recover_controller, interrupted as recover_interrupted,
};

fn path(run: &Path, number: u64, role: Role, kind: StopKind) -> PathBuf {
    let phase = match kind {
        StopKind::Explicit => "stop",
        StopKind::StartupRollback => "rollback",
    };
    directory(run, number).join(format!("role-{phase}-{}.json", role.name()))
}

fn authority<'a>(mode: &Value, role: &'a Role) -> Option<(StopKind, &'a [Role])> {
    match mode.as_str() {
        Some("stop") => Some((StopKind::Explicit, PAIR)),
        Some("close") => Some((StopKind::Explicit, std::slice::from_ref(role))),
        Some("start") if *role == Role::Api => Some((StopKind::StartupRollback, PAIR)),
        _ => None,
    }
}

fn claim_operation(kind: StopKind) -> &'static str {
    match kind {
        StopKind::Explicit => "stop",
        StopKind::StartupRollback => "start",
    }
}

fn receipt_kind(kind: StopKind) -> &'static str {
    match kind {
        StopKind::Explicit => "devex-seed-role-stop-reservation",
        StopKind::StartupRollback => "devex-seed-role-startup-rollback-reservation",
    }
}

fn rollback_authority(
    frame: &Frame<'_>,
    number: u64,
    role: Role,
    intent: &Value,
    token: &Value,
) -> Result<Value> {
    files::absent(&reserved_path(frame.run, number, Role::Worker))?;
    let descriptor = common::binding(&common::common_file(
        frame.backend,
        &reserved_path(frame.run, number, role),
        "回滚对应的原启动预留",
    )?)?;
    reservation(frame, number, role, intent, token)?;
    common::bound_file(frame.backend, &descriptor, "回滚对应的原启动预留后像")?;
    Ok(descriptor)
}

pub(in crate::recovery::clone::native::seed::pair_entry) fn reserve(
    frame: &Frame<'_>,
    roles: &[Role],
    kind: StopKind,
    role: Role,
    token: &str,
    identity: &Value,
    id: &str,
) -> Result<()> {
    let row = state::running_attempt(&frame.saved)?.ok_or("停止预留缺少当前阶段")?;
    if authority(&row["mode"], &role) != Some((kind, roles))
        || !roles.contains(&role)
        || !hex_id(token)
        || !hex_id(id)
        || token == id
    {
        return Err("原停止操作或被停角色代次无效".into());
    }
    common::validate_process_identity(identity)?;
    frame.check()?;
    let intent = common::binding(&common::common_file(
        frame.backend,
        &claim_path(frame.run, frame.number, claim_operation(kind), roles),
        "seed 原停止授权",
    )?)?;
    let rollback = (kind == StopKind::StartupRollback)
        .then(|| rollback_authority(frame, frame.number, role, &intent, &json!(token)))
        .transpose()?;
    document::write_new(
        &path(frame.run, frame.number, role, kind),
        &json!({
            "format_version":1,"kind":receipt_kind(kind),"intent":intent,
            "role":role.name(),"operation_id":id,"start_id":token,"identity":identity,
        }),
    )?;
    common::bound_file(frame.backend, &intent, "seed 原停止授权后像")?;
    if let Some(descriptor) = &rollback {
        common::bound_file(frame.backend, descriptor, "回滚对应的原启动预留后像")?;
        files::absent(&reserved_path(frame.run, frame.number, Role::Worker))?;
    }
    frame.check()
}

struct Reservation {
    kind: StopKind,
    close: bool,
    rollback: Option<Value>,
    number: u64,
    binding: Value,
    intent: Value,
    executor: Value,
    id: String,
    start_id: String,
    identity: Value,
}

fn read_reservation(
    frame: &Frame<'_>,
    row: &Value,
    role: Role,
    active: Option<&history::Active>,
) -> Result<Option<Reservation>> {
    let number = row["number"].as_u64().ok_or("原停止阶段序号无效")?;
    let Some((kind, roles)) = authority(&row["mode"], &role) else {
        return Ok(None);
    };
    let path = path(frame.run, number, role, kind);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    let binding = common::binding(&common::common_file(frame.backend, &path, "原停止预留")?)?;
    let saved = common::read_bound(frame.backend, &binding, "原停止预留")?;
    document::exact(
        &saved,
        &[
            "format_version",
            "kind",
            "intent",
            "role",
            "operation_id",
            "start_id",
            "identity",
        ],
        "原停止预留",
    )?;
    let start_id = document::text(&saved["start_id"], "被停角色原启动编号")?;
    if active.is_some_and(|active| start_id != active.token) {
        return Ok(None);
    }
    let id = document::text(&saved["operation_id"], "原停止操作编号")?;
    let identity = common::validate_process_identity(&saved["identity"])?;
    let intent = common::binding(&common::common_file(
        frame.backend,
        &claim_path(frame.run, number, claim_operation(kind), roles),
        "原停止授权",
    )?)?;
    if saved["format_version"] != 1
        || saved["kind"] != receipt_kind(kind)
        || saved["intent"] != intent
        || saved["role"] != role.name()
        || active.is_some_and(|active| identity != active.identity)
        || !hex_id(id)
        || !hex_id(start_id)
        || id == start_id
    {
        return Err("原停止预留与持久授权或原角色身份不同".into());
    }
    let claim = common::read_bound(frame.backend, &intent, "原停止授权")?;
    verify_claim(frame, row, &claim, claim_operation(kind), roles)?;
    let rollback = (kind == StopKind::StartupRollback)
        .then(|| rollback_authority(frame, number, role, &intent, &saved["start_id"]))
        .transpose()?;
    let executor = common::validate_process_identity(&claim["executor"])?;
    Ok(Some(Reservation {
        kind,
        close: row["mode"] == "close",
        rollback,
        number,
        binding,
        intent,
        executor,
        id: id.to_owned(),
        start_id: start_id.to_owned(),
        identity,
    }))
}

pub(super) struct PendingStop {
    id: String,
    active: history::Active,
    protected: Inputs,
}

impl PendingStop {
    pub(super) fn original<'a>(&'a self, owner: &'a Value) -> Result<controller::ArchivedStop<'a>> {
        self.protected.check()?;
        Ok(controller::ArchivedStop {
            executor: &owner["identity"],
            id: &self.id,
            generation: controller::StopGeneration {
                start_id: &self.active.token,
                identity: &self.active.identity,
            },
        })
    }
}

pub(super) fn verify_pending(
    frame: &Frame<'_>,
    row: &Value,
    role: Role,
    owner: &Value,
) -> Result<Option<PendingStop>> {
    let number = row["number"].as_u64().ok_or("原停止阶段序号无效")?;
    let Some((kind, _)) = authority(&row["mode"], &role) else {
        return Ok(None);
    };
    match std::fs::symlink_metadata(path(frame.run, number, role, kind)) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    let active = history::active(frame.backend, &frame.product, &frame.runtime.runtime, role)?
        .ok_or("原待定停止缺少已记录的启动代次")?;
    let saved =
        read_reservation(frame, row, role, Some(&active))?.ok_or("原待定停止与启动代次不同")?;
    if row["status"] != "failed"
        || saved.id != owner["operation_id"]
        || saved.executor != owner["identity"]
    {
        return Err("原待定停止不同于预留操作及执行者".into());
    }
    let protected = Inputs::open(
        [&saved.intent, &saved.binding]
            .into_iter()
            .chain(saved.rollback.as_ref())
            .map(|binding| common::bound_file(frame.backend, binding, "待定停止原授权"))
            .collect::<Result<Vec<_>>>()?,
    )?;
    Ok(Some(PendingStop {
        id: saved.id,
        active,
        protected,
    }))
}

pub(super) fn resolve(frame: &Frame<'_>, role: Role) -> Result<Option<(Value, Option<Value>)>> {
    let runtime = &frame.runtime.runtime;
    let Some(active) = history::active(frame.backend, &frame.product, runtime, role)? else {
        return resolve_projected(frame, role);
    };
    let mut selected = None;
    for row in state::attempts(&frame.saved)?.iter().filter(|row| {
        row["stage"] == "seed-runtime"
            && row["status"] == "failed"
            && row["number"]
                .as_u64()
                .is_some_and(|number| number < frame.number)
    }) {
        if let Some(reservation) = read_reservation(frame, row, role, Some(&active))?
            && selected.replace(reservation).is_some()
        {
            return Err("原启动代次有多个停止预留，保留现场".into());
        }
    }
    let Some(reservation) = selected else {
        return Ok(None);
    };
    let evidence = reservation
        .close
        .then(|| close_evidence(frame, role, &reservation))
        .transpose()?;
    let proof = project(frame, role, &active, &reservation, evidence.as_ref())?
        .ok_or("已预留的原停止没有确定归档，保留历史和现场")?;
    if let Some(evidence) = &evidence {
        close_files(frame, evidence)?;
    }
    Ok(Some((proof, evidence)))
}

fn resolve_projected(frame: &Frame<'_>, role: Role) -> Result<Option<(Value, Option<Value>)>> {
    let Some((evidence, marker)) = projection::prior(frame, role)? else {
        return Ok(None);
    };
    let number = evidence["close_attempt"]
        .as_u64()
        .filter(|number| *number < frame.number)
        .ok_or("Recover 投影标记缺少原 Close 阶段")?;
    let row = state::attempts(&frame.saved)?
        .iter()
        .find(|row| row["number"].as_u64() == Some(number))
        .ok_or("Recover 投影标记不属于本运行的原 Close 阶段")?;
    let reservation = close_reservation(frame, row, role, &evidence)?;
    let marker_path = common::bound_file(frame.backend, &marker, "原 Recover 投影标记")?;
    let protected = Inputs::open([marker_path])?;
    let result = read_projected(frame, role, &reservation, &evidence, || {
        protected.check()?;
        common::bound_file(frame.backend, &marker, "原 Recover 投影标记后像")?;
        Ok(())
    })?;
    Ok(Some((result, Some(evidence))))
}

fn project(
    frame: &Frame<'_>,
    role: Role,
    active: &history::Active,
    reservation: &Reservation,
    evidence: Option<&Value>,
) -> Result<Option<Value>> {
    let runtime = &frame.runtime.runtime;
    let history = common::binding(&runtime.join("producer-history.json"))?;
    let check = || -> Result<()> {
        frame.check()?;
        for descriptor in [&reservation.intent, &reservation.binding, &history] {
            common::bound_file(frame.backend, descriptor, "原停止归档授权后像")?;
        }
        if let Some(descriptor) = &reservation.rollback {
            common::bound_file(frame.backend, descriptor, "原回滚的启动授权后像")?;
            files::absent(&reserved_path(frame.run, reservation.number, Role::Worker))?;
        }
        if let Some(evidence) = evidence {
            close_files(frame, evidence)?;
        }
        Ok(())
    };
    check()?;
    controller::archived_stop_bound(
        role,
        &frame.product,
        runtime,
        &frame.runtime.handoff["api_environment"],
        &controller::ArchivedStop {
            executor: &reservation.executor,
            id: &reservation.id,
            generation: controller::StopGeneration {
                start_id: &active.token,
                identity: &active.identity,
            },
        },
        || {
            let producer = Producer::new(
                frame.backend,
                runtime,
                &frame.contract,
                role,
                &active.token,
                &active.ready_url,
            )?;
            let marker = evidence
                .map(|evidence| projection::record(frame, role, evidence))
                .transpose()?;
            let marker_path = marker
                .as_ref()
                .map(|marker| common::bound_file(frame.backend, marker, "当前 Recover 投影标记"))
                .transpose()?;
            let protected = Inputs::open(marker_path)?;
            reservation.kind.publish(&producer, &active.identity, || {
                check()?;
                protected.check()
            })?;
            protected.check()?;
            if let Some(marker) = marker {
                common::bound_file(frame.backend, &marker, "当前 Recover 投影标记后像")?;
            }
            Ok(())
        },
    )
}

fn close_evidence(frame: &Frame<'_>, role: Role, reservation: &Reservation) -> Result<Value> {
    let archive = frame
        .runtime
        .runtime
        .join(role.archive())
        .join(&reservation.id);
    let mut files = serde_json::Map::new();
    for name in ["owner", "request", "result"] {
        let path = archive.join(format!("{name}.json"));
        let binding = common::binding(&common::common_file(frame.backend, &path, "原停止归档")?)?;
        files.insert(name.into(), binding);
    }
    Ok(
        json!({"format_version":1,"kind":"devex-seed-close-stop-recovery",
        "close_attempt":reservation.number,"handoff":frame.handoff,"reservation":reservation.binding,
        "intent":reservation.intent,"operation_id":reservation.id,"archive":files}),
    )
}

fn close_files(frame: &Frame<'_>, evidence: &Value) -> Result<Vec<Value>> {
    let files = [
        &evidence["reservation"],
        &evidence["intent"],
        &evidence["archive"]["owner"],
        &evidence["archive"]["request"],
        &evidence["archive"]["result"],
    ];
    for descriptor in files {
        common::bound_file(frame.backend, descriptor, "close 停止归档原字节")?;
    }
    Ok(files.into_iter().cloned().collect())
}

pub(in crate::recovery::clone::native::seed::pair_entry) fn verify_recovered_close_stop(
    frame: &Frame<'_>,
    row: &Value,
    role: Role,
    identity: &Value,
    result: &Value,
    evidence: &Value,
) -> Result<Vec<Value>> {
    frame.check()?;
    let reservation = close_reservation(frame, row, role, evidence)?;
    if reservation.identity != *identity {
        return Err("恢复收据的原角色身份与 Close 检查点不同".into());
    }
    let archive = read_projected(frame, role, &reservation, evidence, || Ok(()))?;
    if archive != *result {
        return Err("恢复结果不是同一原角色成功停止归档".into());
    }
    close_files(frame, evidence)
}

fn close_reservation(
    frame: &Frame<'_>,
    row: &Value,
    role: Role,
    evidence: &Value,
) -> Result<Reservation> {
    if row["stage"] != "seed-runtime" || row["mode"] != "close" || row["status"] != "failed" {
        return Err("恢复收据不属于原失败 Close".into());
    }
    let reservation = read_reservation(frame, row, role, None)?.ok_or("原 Close 缺少停止预留")?;
    if !reservation.close
        || reservation.kind != StopKind::Explicit
        || close_evidence(frame, role, &reservation)? != *evidence
    {
        return Err("恢复收据与原 Close 停止预留或归档字节不同".into());
    }
    Ok(reservation)
}

fn read_projected(
    frame: &Frame<'_>,
    role: Role,
    reservation: &Reservation,
    evidence: &Value,
    check: impl Fn() -> Result<()>,
) -> Result<Value> {
    let checked = || {
        close_files(frame, evidence)?;
        check()
    };
    checked()?;
    let archive = controller::archived_stop_bound(
        role,
        &frame.product,
        &frame.runtime.runtime,
        &frame.runtime.handoff["api_environment"],
        &controller::ArchivedStop {
            executor: &reservation.executor,
            id: &reservation.id,
            generation: controller::StopGeneration {
                start_id: &reservation.start_id,
                identity: &reservation.identity,
            },
        },
        || {
            checked()?;
            verify_projected_stop(frame, role, reservation)
        },
    )?
    .ok_or("已投影停止缺少原角色归档")?;
    checked()?;
    frame.check()?;
    Ok(archive)
}

fn verify_projected_stop(frame: &Frame<'_>, role: Role, reservation: &Reservation) -> Result<()> {
    let collected =
        super::super::super::source_registration::lineage::runtime::collect_for_product(
            frame.backend,
            &frame.product,
            &frame.runtime.runtime,
        )?;
    let original = common::read_bound(frame.backend, &collected["history"], "原已投影停止历史")?;
    let events = document::array(&original["events"], "原生产者历史事件")?;
    let mut role_events = events
        .iter()
        .rev()
        .filter(|event| event["role"] == role.name());
    let stop = role_events.next().ok_or("恢复未投影原角色停止历史")?;
    let start = role_events.next().ok_or("恢复停止历史缺少原启动代次")?;
    if stop["event"] != "stopped"
        || stop["identity"] != reservation.identity
        || start["event"] != "started"
        || start["token"] != reservation.start_id
        || start["identity"] != reservation.identity
    {
        return Err("恢复历史未绑定原 Close 的精确启动与停止事件".into());
    }
    common::bound_file(frame.backend, &collected["history"], "原已投影停止历史后像")?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/recovery/clone/native/seed/pair_entry/projection_chain.rs"]
mod projection_chain_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_uses_original_api_start_claim_and_distinct_stop_receipt() {
        let role = Role::Api;
        let (kind, roles) = authority(&json!("start"), &role).unwrap();
        assert!(kind == StopKind::StartupRollback);
        assert_eq!(roles, PAIR);
        assert_eq!(claim_operation(kind), "start");
        assert!(authority(&json!("start"), &Role::Worker).is_none());
        let run = Path::new("run");
        assert_ne!(
            path(run, 7, role, kind),
            path(run, 7, role, StopKind::Explicit)
        );
        assert_ne!(receipt_kind(kind), receipt_kind(StopKind::Explicit));
    }
}
