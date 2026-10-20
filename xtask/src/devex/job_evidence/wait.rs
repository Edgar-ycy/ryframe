use std::{
    collections::BTreeMap,
    io::BufRead,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::{
    model,
    protocol::{self, EvidenceResult, Session},
    sql,
};

pub(crate) struct Pending {
    pub(crate) selection: Value,
    pub(crate) deadline: Duration,
}

pub(crate) fn accept(
    value: &Value,
    pending: &mut BTreeMap<u64, Pending>,
    last: &mut u64,
    now: Duration,
) -> EvidenceResult<()> {
    protocol::exact(
        value,
        &["ticket", "selection", "timeout_ms"],
        "invalid_wait_request",
    )?;
    let ticket = value["ticket"]
        .as_u64()
        .filter(|value| *last < *value && *value <= 4_000_000)
        .ok_or("invalid_wait_request")?;
    let timeout = value["timeout_ms"]
        .as_u64()
        .filter(|value| (1000..=300_000).contains(value))
        .ok_or("invalid_wait_request")?;
    if pending.len() >= 100 {
        return Err("invalid_wait_request");
    }
    model::selection(&value["selection"])?;
    let deadline = now
        .checked_add(Duration::from_millis(timeout))
        .ok_or("invalid_wait_request")?;
    *last = ticket;
    pending.insert(
        ticket,
        Pending {
            selection: value["selection"].clone(),
            deadline,
        },
    );
    Ok(())
}

pub(crate) fn poll(
    session: &mut impl Session,
    binding: &Value,
    scope: &str,
    pending: &mut BTreeMap<u64, Pending>,
    mut now: impl FnMut() -> Duration,
    emit: &mut impl FnMut(Value) -> EvidenceResult<()>,
) -> EvidenceResult<()> {
    session.query("START TRANSACTION READ ONLY")?;
    let result = (|| {
        protocol::owner(session, binding, scope)?;
        let database = binding["control"]["database"]
            .as_str()
            .ok_or("invalid_control_binding")?;
        let tickets = pending.keys().copied().collect::<Vec<_>>();
        for ticket in tickets {
            let item = pending.get(&ticket).ok_or("invalid_wait_request")?;
            let mut result = if now() >= item.deadline {
                None
            } else {
                let rows = session.query(&sql::job_sql(database, &item.selection)?)?;
                model::completion(&rows, &item.selection)
            };
            if now() >= item.deadline {
                result = Some(json!({"valid": false, "reason": "job_completion_timeout"}));
            }
            if let Some(result) = result {
                emit(response(ticket, scope, &item.selection, result)?)?;
                pending.remove(&ticket);
            }
        }
        Ok(())
    })();
    let rollback = session.query("ROLLBACK").map(|_| ());
    result.and(rollback)
}

fn response(
    ticket: u64,
    scope: &str,
    selection: &Value,
    mut result: Value,
) -> EvidenceResult<Value> {
    let object = result.as_object_mut().ok_or("database_completion_failed")?;
    object.insert("ticket".into(), json!(ticket));
    object.insert("scope_id".into(), json!(scope));
    object.insert("selection".into(), selection.clone());
    Ok(result)
}

pub(crate) fn serve(
    session: &mut impl Session,
    binding: &Value,
    scope: &str,
    reader: impl BufRead + Send + 'static,
    mut emit: impl FnMut(Value) -> EvidenceResult<()>,
) -> EvidenceResult<()> {
    session.query("SET SESSION time_zone='+00:00'")?;
    protocol::owner(session, binding, scope)?;
    emit(json!({"ready": true, "scope_id": scope}))?;
    let receiver = requests(reader);
    run(session, binding, scope, &receiver, &mut emit)
}

fn requests(mut reader: impl BufRead + Send + 'static) -> Receiver<EvidenceResult<Option<Value>>> {
    let (sender, receiver) = mpsc::sync_channel(101);
    // 只读 stdin 的线程不持有数据库；接收端退出时有界发送解除，不积累未知请求。
    std::thread::spawn(move || {
        loop {
            let request = protocol::read_line(&mut reader);
            let terminal = !matches!(&request, Ok(Some(_)));
            if sender.send(request).is_err() || terminal {
                break;
            }
        }
    });
    receiver
}

fn run(
    session: &mut impl Session,
    binding: &Value,
    scope: &str,
    receiver: &Receiver<EvidenceResult<Option<Value>>>,
    emit: &mut impl FnMut(Value) -> EvidenceResult<()>,
) -> EvidenceResult<()> {
    let started = Instant::now();
    let mut pending = BTreeMap::new();
    let mut last_ticket = 0;
    let mut next_poll = Duration::ZERO;
    loop {
        let wait = if pending.is_empty() {
            Duration::from_millis(250)
        } else {
            next_poll
                .saturating_sub(started.elapsed())
                .max(Duration::from_millis(1))
        };
        match receiver.recv_timeout(wait) {
            Ok(Ok(Some(value))) => {
                accept(&value, &mut pending, &mut last_ticket, started.elapsed())?
            }
            Ok(Ok(None)) | Err(RecvTimeoutError::Disconnected) => {
                return input_closed(session, binding, scope, &pending, emit);
            }
            Ok(Err(reason)) => return Err(reason),
            Err(RecvTimeoutError::Timeout) => {}
        }
        // 新到请求先按顺序登记；每轮使用新的只读快照，不能复用旧 REPEATABLE READ。
        while let Ok(request) = receiver.try_recv() {
            match request? {
                Some(value) => accept(&value, &mut pending, &mut last_ticket, started.elapsed())?,
                None => {
                    return input_closed(session, binding, scope, &pending, emit);
                }
            }
        }
        if !pending.is_empty() && started.elapsed() >= next_poll {
            poll(
                session,
                binding,
                scope,
                &mut pending,
                || started.elapsed(),
                emit,
            )?;
            next_poll = started.elapsed() + Duration::from_millis(250);
        }
    }
}

fn input_closed(
    session: &mut impl Session,
    binding: &Value,
    scope: &str,
    pending: &BTreeMap<u64, Pending>,
    emit: &mut impl FnMut(Value) -> EvidenceResult<()>,
) -> EvidenceResult<()> {
    for (ticket, item) in pending {
        emit(response(
            *ticket,
            scope,
            &item.selection,
            json!({"valid": false, "reason": "job_observer_input_closed"}),
        )?)?;
    }
    protocol::owner(session, binding, scope)
}
