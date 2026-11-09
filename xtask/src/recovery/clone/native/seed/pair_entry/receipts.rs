//! 每个角色动作只消费一次授权；恢复只投影可证明的原操作。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};

use super::{Frame, PAIR, Role, allowed, common, controller, document, state};
use crate::{
    Result,
    recovery::clone::native::cache::handoff::history::{self, Producer},
};

#[path = "receipts/archived.rs"]
mod archived;
#[path = "receipts/archived_stop.rs"]
mod archived_stop;
pub(super) use archived_stop::reserve as reserve_stop;
pub(super) use archived_stop::{
    recover_controller, recover_interrupted, verify_recovered_close_stop,
};

fn directory(run: &Path, number: u64) -> PathBuf {
    run.join(format!("seed-runtime/attempt-{number:04}"))
}

fn claim_path(run: &Path, number: u64, operation: &str, roles: &[Role]) -> PathBuf {
    let roles = roles
        .iter()
        .map(|role| role.name())
        .collect::<Vec<_>>()
        .join("-");
    directory(run, number).join(format!("role-control-{operation}-{roles}.json"))
}

pub(super) fn claim(frame: &Frame<'_>, operation: &str, roles: &[Role]) -> Result<()> {
    frame.check()?;
    let directory = directory(frame.run, frame.number);
    if !directory.try_exists()? {
        fs::create_dir(&directory)?;
    }
    common::local_directory(frame.backend, &directory, "seed 当前角色收据目录")?;
    let roles_value = roles.iter().map(|role| role.name()).collect::<Vec<_>>();
    document::write_new(
        &claim_path(frame.run, frame.number, operation, roles),
        &json!({"format_version":1,"kind":"devex-seed-role-control-intent","attempt":frame.number,
            "controller":frame.controller,"executor":frame.executor,"handoff":frame.handoff,
            "operation":operation,"roles":roles_value}),
    )?;
    frame.check()
}

fn reserved_path(run: &Path, number: u64, role: Role) -> PathBuf {
    directory(run, number).join(format!("role-start-{}.json", role.name()))
}

pub(super) fn reserved(frame: &Frame<'_>, role: Role, id: &str) -> Result<()> {
    if !hex_id(id) {
        return Err("seed 角色预留编号无效".into());
    }
    frame.check()?;
    let path = claim_path(frame.run, frame.number, "start", PAIR);
    let intent = common::binding(&common::common_file(
        frame.backend,
        &path,
        "seed 当前启动意图",
    )?)?;
    document::write_new(
        &reserved_path(frame.run, frame.number, role),
        &json!({"format_version":1,"kind":"devex-seed-role-start-reservation","intent":intent,
            "role":role.name(),"operation_id":id}),
    )?;
    frame.check()
}

pub(super) fn recover(frame: &Frame<'_>, roles: &[Role]) -> Result<Value> {
    let mut result = Map::new();
    for role in roles {
        frame.check()?;
        let filename = frame
            .runtime
            .runtime
            .join(format!("{}-control.lock/owner.json", role.name()));
        let pending = match fs::symlink_metadata(&filename) {
            Ok(_) => common::binding(&common::common_file(
                frame.backend,
                &filename,
                "原待定角色 owner",
            )?)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let lock = filename.parent().ok_or("缺少角色控制目录")?;
                crate::recovery::clone::native::post_process::files::absent(lock)?;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let owner = common::read_bound(frame.backend, &pending, "原待定角色 owner")?;
        let (operation, intent, reservation, stop) = original(frame, *role, &owner)?;
        common::bound_file(frame.backend, &pending, "原角色 owner 后像")?;
        common::bound_file(frame.backend, &intent, "原角色操作后像")?;
        frame.check()?;
        let runtime = &frame.runtime.runtime;
        let environment = &frame.runtime.handoff["api_environment"];
        let value = if let Some(stop) = &stop {
            controller::reconcile_recorded_stop(
                *role,
                &frame.product,
                runtime,
                environment,
                &stop.original(&owner)?,
            )
        } else {
            controller::reconcile_completed_bound(
                *role,
                &frame.product,
                runtime,
                environment,
                &owner["identity"],
                operation,
                reservation.as_deref(),
            )
        }?
        .ok_or("原待定角色在精确恢复期间变化")?;
        if stop.is_none() {
            project(frame, *role, operation, reservation.as_deref(), &value)?;
        }
        result.insert(role.name().into(), value);
    }
    if let Some((role, value)) = archived::resolve(frame)? {
        result.insert(role.name().into(), value);
    }
    let mut close_stops = Map::new();
    for role in roles {
        if let Some((value, evidence)) = archived_stop::resolve(frame, *role)? {
            result.insert(role.name().into(), value);
            if let Some(evidence) = evidence {
                close_stops.insert(role.name().into(), evidence);
            }
        }
    }
    frame.check()?;
    let mut value = json!({"format_version":1,"kind":"devex-clone-runtime-lock-reconciliation",
        "roles":result,"remote_writes_reconciled":false});
    if !close_stops.is_empty() {
        value["close_stop_evidence"] = Value::Object(close_stops);
    }
    Ok(value)
}

fn project(
    frame: &Frame<'_>,
    role: Role,
    operation: &str,
    id: Option<&str>,
    value: &Value,
) -> Result<()> {
    let runtime = &frame.runtime.runtime;
    frame.check()?;
    if operation == "start" {
        let ready = match role {
            Role::Api => format!(
                "{}/readyz",
                document::text(&frame.runtime.handoff["api_url"], "seed API 地址")?
                    .trim_end_matches('/')
            ),
            Role::Worker => {
                document::text(&frame.contract["worker_ready_url"], "seed Worker 地址")?.to_owned()
            }
            Role::Frontend => return Err("seed 不控制前端角色".into()),
        };
        Producer::new(
            frame.backend,
            runtime,
            &frame.contract,
            role,
            id.ok_or("恢复启动缺少原预留")?,
            &ready,
        )?
        .started(&value["identity"])?;
    } else if let Some(active) = history::active(frame.backend, &frame.product, runtime, role)? {
        Producer::new(
            frame.backend,
            runtime,
            &frame.contract,
            role,
            &active.token,
            &active.ready_url,
        )?
        .stopped(&active.identity, || frame.check())?;
    }
    frame.check()
}

fn original(
    frame: &Frame<'_>,
    role: Role,
    owner: &Value,
) -> Result<(
    &'static str,
    Value,
    Option<String>,
    Option<archived_stop::PendingStop>,
)> {
    common::validate_process_identity(&owner["identity"])?;
    let mut matched = None;
    for row in state::attempts(&frame.saved)?.iter().rev().filter(|row| {
        row["stage"] == "seed-runtime"
            && row["number"]
                .as_u64()
                .is_some_and(|number| number < frame.number)
    }) {
        let Some((operation, roles)) = (match row["mode"].as_str() {
            Some("start") => Some(("start", PAIR)),
            Some("stop") => Some(("stop", PAIR)),
            Some("close") => Some(("stop", std::slice::from_ref(&role))),
            _ => None,
        }) else {
            continue;
        };
        let number = row["number"].as_u64().ok_or("原 seed 控制阶段序号无效")?;
        let path = claim_path(frame.run, number, operation, roles);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        let binding = common::binding(&common::common_file(
            frame.backend,
            &path,
            "原角色控制意图",
        )?)?;
        let claim = common::read_bound(frame.backend, &binding, "原角色控制意图")?;
        if claim["executor"] != owner["identity"] {
            continue;
        }
        verify_claim(frame, row, &claim, operation, roles)?;
        let stop = archived_stop::verify_pending(frame, row, role, owner)?;
        let operation = if stop.is_some() { "stop" } else { operation };
        let reservation = if operation == "start" {
            Some(reservation(
                frame,
                number,
                role,
                &binding,
                &owner["operation_id"],
            )?)
        } else {
            None
        };
        if matched
            .replace((operation, binding, reservation, stop))
            .is_some()
        {
            return Err("原待定角色对应多个控制意图，保留现场".into());
        }
    }
    matched.ok_or_else(|| "原待定角色没有明确本 run 的持久意图".into())
}

fn verify_claim(
    frame: &Frame<'_>,
    row: &Value,
    claim: &Value,
    operation: &str,
    roles: &[Role],
) -> Result<()> {
    let mode = document::text(&row["mode"], "原角色控制阶段")?;
    allowed(mode, operation, roles)?;
    let number = row["number"].as_u64().ok_or("原角色控制序号无效")?;
    let mut running = row.clone();
    running["status"] = json!("running");
    for field in ["finished_at", "result", "error_type"] {
        running[field] = Value::Null;
    }
    let (binding, _) = state::controller_record(frame.run, number, &running)?;
    if *claim
        != json!({"format_version":1,"kind":"devex-seed-role-control-intent","attempt":number,
        "controller":binding,"executor":claim["executor"],"handoff":frame.handoff,"operation":operation,
        "roles":roles.iter().map(|role| role.name()).collect::<Vec<_>>()})
    {
        return Err("原角色意图不属于同一阶段和已发布交接".into());
    }
    Ok(())
}

fn reservation(
    frame: &Frame<'_>,
    number: u64,
    role: Role,
    intent: &Value,
    operation: &Value,
) -> Result<String> {
    let path = reserved_path(frame.run, number, role);
    let binding = common::binding(&common::common_file(frame.backend, &path, "原启动预留")?)?;
    let value = common::read_bound(frame.backend, &binding, "原启动预留")?;
    let id = document::text(operation, "原角色操作编号")?;
    if !hex_id(id)
        || value
            != json!({"format_version":1,"kind":"devex-seed-role-start-reservation",
        "intent":intent,"role":role.name(),"operation_id":id})
    {
        return Err("原待定启动不同于持久预留".into());
    }
    Ok(id.to_owned())
}

fn hex_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
