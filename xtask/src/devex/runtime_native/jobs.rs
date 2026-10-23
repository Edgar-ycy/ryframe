use super::mysql::Mysql;
use super::{
    artifacts, configuration,
    failure::{Failure, NativeResult},
    process::Cancellation,
};
use crate::devex::job_evidence::{
    self,
    protocol::{self, Session},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(crate) fn selection(workflow: &Value, variables: &Value) -> NativeResult<Value> {
    let job = configuration::expand(&workflow["job"], variables)?;
    let value = json!({"kind":job["kind"],"id":job["id"],"job_type":job["job_type"],"tenant_id":variables["tenant_id"]});
    job_evidence::model::selection(&value).map_err(|_| Failure::new("job_selection"))?;
    Ok(value)
}

pub(crate) struct ArtifactGuard {
    binding: Option<job_evidence::artifact::Binding>,
    directory: PathBuf,
}
impl ArtifactGuard {
    pub(crate) fn before(
        config: &Value,
        input: &super::Input,
        directory: &Path,
    ) -> NativeResult<Self> {
        let suite = &input.suite;
        let needed = config["contract"]["workloads"][suite]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row.get("job").is_some()));
        let binding = if needed {
            let actual = artifact_binding()?;
            if actual != input.native_tool {
                return Err(Failure::new("native_tool_binding"));
            }
            Some(actual)
        } else {
            None
        };
        if let Some(binding) = &binding {
            artifacts::write(&directory.join("native-tool-before.json"), binding)?;
        }
        Ok(Self {
            binding,
            directory: directory.into(),
        })
    }
    pub(crate) fn after(&mut self) -> NativeResult<()> {
        if let Some(before) = &self.binding {
            let after = artifact_binding()?;
            if *before != after {
                return Err(Failure::new("native_tool_changed"));
            }
            artifacts::write(&self.directory.join("native-tool-after.json"), &after)?;
        }
        Ok(())
    }
}
fn artifact_binding() -> NativeResult<job_evidence::artifact::Binding> {
    let path = std::env::current_exe().map_err(|_| Failure::new("native_tool_binding"))?;
    job_evidence::artifact::typed_binding(&path).map_err(|_| Failure::new("native_tool_binding"))
}

struct WaitRequest {
    selection: Value,
    deadline: Instant,
    reply: tokio::sync::oneshot::Sender<Value>,
}
pub(crate) struct Waiter {
    sender: Option<mpsc::SyncSender<WaitRequest>>,
    closed: tokio::sync::oneshot::Receiver<NativeResult<()>>,
    cancellation: Cancellation,
    worker: Option<thread::JoinHandle<()>>,
}

impl Waiter {
    pub(crate) async fn start(config: &Value, cancel: &Cancellation) -> NativeResult<Self> {
        let binding = config["bindings"]["job_timings"].clone();
        let scope = config["bindings"]["scope_id"]
            .as_str()
            .ok_or_else(|| Failure::new("job_scope"))?
            .to_owned();
        let (sender, receiver) = mpsc::sync_channel(100);
        let (ready_send, ready) = tokio::sync::oneshot::channel();
        let (close_send, closed) = tokio::sync::oneshot::channel();
        let cancellation = cancel.linked();
        let owned_cancel = cancellation.clone();
        let worker = thread::spawn(move || {
            let initialized = observer_session(
                &binding,
                &scope,
                cancellation.clone(),
                Duration::from_secs(10),
            );
            let mut session = match initialized {
                Ok(session) => {
                    let _ = ready_send.send(Ok(()));
                    session
                }
                Err(error) => {
                    let _ = ready_send.send(Err(error.clone()));
                    let _ = close_send.send(Err(error));
                    return;
                }
            };
            let result = observer(&mut session, &binding, &scope, receiver, &cancellation);
            let close = session
                .close()
                .map_err(|_| Failure::new("job_observer_close"));
            let _ = close_send.send(result.and(close));
        });
        let started = cancel
            .during(async {
                tokio::time::timeout(Duration::from_secs(15), ready)
                    .await
                    .map_err(|_| Failure::new("job_observer_start_timeout"))?
                    .map_err(|_| Failure::new("job_observer_start_failed"))?
            })
            .await;
        if let Err(error) = started {
            owned_cancel.cancel();
            let _ = worker.join();
            return Err(error);
        }
        Ok(Self {
            sender: Some(sender),
            closed,
            cancellation: owned_cancel,
            worker: Some(worker),
        })
    }
    pub(crate) async fn wait(
        &self,
        selected: Value,
        timeout: Duration,
        cancel: &Cancellation,
    ) -> NativeResult<Value> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.sender
            .as_ref()
            .ok_or_else(|| Failure::new("job_observer_closed"))?
            .try_send(WaitRequest {
                selection: selected,
                deadline: Instant::now() + timeout,
                reply,
            })
            .map_err(|_| Failure::new("job_observer_input_failed"))?;
        cancel
            .during(async {
                tokio::time::timeout(timeout + Duration::from_secs(15), result)
                    .await
                    .map_err(|_| Failure::new("job_observer_response_timeout"))?
                    .map_err(|_| Failure::new("job_observer_invalid_response"))
            })
            .await
    }
    pub(crate) async fn close(mut self) -> NativeResult<()> {
        self.sender.take();
        tokio::time::timeout(Duration::from_secs(15), &mut self.closed)
            .await
            .map_err(|_| Failure::new("job_observer_close_timeout"))?
            .map_err(|_| Failure::new("job_observer_exit_failed"))?
    }
}
impl Drop for Waiter {
    fn drop(&mut self) {
        self.sender.take();
        self.cancellation.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn observer_session(
    binding: &Value,
    scope: &str,
    cancel: Cancellation,
    timeout: Duration,
) -> NativeResult<Mysql> {
    // 持久连接及绑定验证复用原生任务采集器；不另建 SQL 或所有权规则。
    let mut session = Mysql::open(binding, cancel, timeout)
        .map_err(|_| Failure::new("job_observer_start_failed"))?;
    session
        .query("SET SESSION time_zone='+00:00'")
        .map_err(|_| Failure::new("job_observer_start_failed"))?;
    protocol::owner(&mut session, binding, scope)
        .map_err(|_| Failure::new("job_observer_binding"))?;
    Ok(session)
}
fn observer(
    session: &mut impl Session,
    binding: &Value,
    scope: &str,
    receiver: mpsc::Receiver<WaitRequest>,
    cancel: &Cancellation,
) -> NativeResult<()> {
    let origin = Instant::now();
    let mut pending = BTreeMap::new();
    let mut replies = BTreeMap::new();
    let mut ticket = 0_u64;
    loop {
        cancel.check()?;
        match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(request) => {
                ticket += 1;
                let deadline = request.deadline.saturating_duration_since(origin);
                pending.insert(
                    ticket,
                    job_evidence::wait::Pending {
                        selection: request.selection,
                        deadline,
                    },
                );
                replies.insert(ticket, request.reply);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) if pending.is_empty() => {
                return protocol::owner(session, binding, scope)
                    .map_err(|_| Failure::new("job_observer_binding"));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Failure::new("job_observer_input_closed"));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        while let Ok(request) = receiver.try_recv() {
            ticket += 1;
            pending.insert(
                ticket,
                job_evidence::wait::Pending {
                    selection: request.selection,
                    deadline: request.deadline.saturating_duration_since(origin),
                },
            );
            replies.insert(ticket, request.reply);
        }
        if !pending.is_empty() {
            job_evidence::wait::poll(
                session,
                binding,
                scope,
                &mut pending,
                || origin.elapsed(),
                &mut |result| {
                    let ticket = result["ticket"]
                        .as_u64()
                        .ok_or("job_observer_invalid_response")?;
                    if result["scope_id"] != scope {
                        return Err("job_observer_invalid_response");
                    }
                    let reply = replies
                        .remove(&ticket)
                        .ok_or("job_observer_invalid_response")?;
                    let _ = reply.send(result);
                    Ok(())
                },
            )
            .map_err(|_| Failure::new("job_observer_failed"))?;
        }
    }
}

pub(crate) async fn collect(
    config: &Value,
    workflow: &Value,
    variables: &Value,
    directory: &Path,
    cancel: &Cancellation,
) -> NativeResult<(f64, f64)> {
    let started = Instant::now();
    let selected = selection(workflow, variables);
    let timeout = Duration::from_millis(config["contract"]["timeout_ms"].as_u64().unwrap_or(0));
    let mut result = json!({"valid":false,"reason":"collector_execution_failed"});
    if let Ok(selected) = &selected {
        loop {
            cancel.check()?;
            let binding = config["bindings"]["job_timings"].clone();
            let scope = config["bindings"]["scope_id"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            let selection = selected.clone();
            let (send, receive) = tokio::sync::oneshot::channel();
            let owned_cancel = cancel.linked();
            let worker_cancel = owned_cancel.clone();
            let worker = thread::spawn(move || {
                let value = (|| {
                    let mut session = observer_session(
                        &binding,
                        &scope,
                        worker_cancel,
                        timeout.min(Duration::from_secs(60)),
                    )?;
                    let value = protocol::collect(&mut session, &binding, &scope, &selection)
                        .unwrap_or_else(|reason| json!({"valid":false,"reason":reason}));
                    let closed = session
                        .close()
                        .map_err(|_| Failure::new("collector_execution_failed"));
                    closed?;
                    Ok(value)
                })();
                let _ = send.send(value);
            });
            let response = cancel
                .during(async {
                    tokio::time::timeout(timeout.min(Duration::from_secs(60)), receive)
                        .await
                        .map_err(|_| Failure::new("collector_timeout"))?
                        .map_err(|_| Failure::new("collector_execution_failed"))?
                })
                .await;
            if response.is_err() {
                owned_cancel.cancel();
            }
            let joined = worker.join();
            result =
                response.unwrap_or_else(|error| json!({"valid":false,"reason":error.category}));
            if joined.is_err() {
                result = json!({"valid":false,"reason":"collector_execution_failed"});
            }
            if !matches!(
                result["reason"].as_str(),
                Some("missing_job" | "message_not_dispatched" | "job_unfinished")
            ) || started.elapsed() >= timeout
            {
                break;
            }
            cancel.sleep(Duration::from_millis(250)).await?;
        }
        if result["valid"] == true
            && (result["scope_id"] != config["bindings"]["scope_id"]
                || result["kind"] != selected["kind"]
                || result["business_id"] != selected["id"]
                || ["queue_ms", "execution_ms"].iter().any(|key| {
                    result[*key]
                        .as_f64()
                        .is_none_or(|v| !v.is_finite() || v < 0.0)
                }))
        {
            result = json!({"valid":false,"reason":"collector_binding_mismatch"});
        }
    }
    let mut evidence = json!({"workflow":workflow["name"],"worker":variables["worker"],"cycle":variables["cycle"],
        "business_duration_ms":variables["business_duration_ms"],"business_succeeded":variables["business_succeeded"],
        "request_duration_ms":variables["request_duration_ms"],"completion":variables["completion"],
        "collection_duration_ms":super::milliseconds(started.elapsed()),"selection":selected.as_ref().ok()});
    evidence
        .as_object_mut()
        .ok_or_else(|| Failure::new("job_evidence"))?
        .extend(
            result
                .as_object()
                .ok_or_else(|| Failure::new("job_evidence"))?
                .clone(),
        );
    artifacts::append(&directory.join("job-attempts.jsonl"), &[evidence])?;
    if result["valid"] != true {
        return Err(Failure::new("job_timing_evidence_invalid"));
    }
    Ok((
        result["queue_ms"]
            .as_f64()
            .ok_or_else(|| Failure::new("job_timing_evidence_invalid"))?,
        result["execution_ms"]
            .as_f64()
            .ok_or_else(|| Failure::new("job_timing_evidence_invalid"))?,
    ))
}
