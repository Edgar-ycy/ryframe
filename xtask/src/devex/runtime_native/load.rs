use super::{
    Input, artifacts, configuration,
    failure::{Failure, NativeResult},
    imports::Samples,
    jobs,
    pacing::Pacing,
    process::Cancellation,
    request::{RequestObservation, Session},
};
use futures_util::future::join_all;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Instant};

pub(crate) fn quantile(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() as f64 * fraction).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1)]
}
pub(crate) fn summarize(
    durations: &[f64],
    failed: u64,
    elapsed: f64,
    queues: &[f64],
    executions: &[f64],
) -> Value {
    json!({"completed":durations.len(),"failed":failed,"elapsed_ms":elapsed,"p50_ms":quantile(durations,0.5),
        "p95_ms":quantile(durations,0.95),"p99_ms":quantile(durations,0.99),
        "throughput":if elapsed>0.0 { durations.len() as f64/(elapsed/1000.0) }else{0.0},
        "queue_p95_ms":(!queues.is_empty()).then(||quantile(queues,0.95)),
        "execution_p95_ms":(!executions.is_empty()).then(||quantile(executions,0.95))})
}

pub(crate) struct Cycle {
    pub variables: Value,
    pub succeeded: bool,
    pub request_elapsed: f64,
    pub elapsed: f64,
    pub completion: Option<Value>,
    pub proof: bool,
    pub failure: Option<Value>,
    pub queue: Option<f64>,
    pub execution: Option<f64>,
}
pub(crate) struct Pending {
    pub workflow: Value,
    pub records: Vec<Cycle>,
    pub failed: u64,
    pub elapsed: f64,
    pub events: Vec<Value>,
    pub http_requests: Vec<Vec<RequestObservation>>,
}
pub(crate) struct Measurement {
    pub scenarios: BTreeMap<String, Value>,
    pub network_requests: u64,
    pub completed_cycles: u64,
    pub failed_cycles: u64,
    pub session_failures: u64,
    pub pending: Vec<Pending>,
}

impl Measurement {
    pub(crate) fn document(&self) -> Value {
        json!({"scenarios":self.scenarios,"network_requests":self.network_requests,"completed_cycles":self.completed_cycles,
            "failed_cycles":self.failed_cycles,"session_failures":self.session_failures})
    }
    pub(crate) async fn finalize(
        &mut self,
        config: &Value,
        directory: &Path,
        cancel: &Cancellation,
    ) -> NativeResult<()> {
        for item in &mut self.pending {
            if item.workflow.get("job").is_some() {
                for row in &mut item.records {
                    let mut variables = row.variables.clone();
                    variables["business_duration_ms"] = json!(row.elapsed);
                    variables["business_succeeded"] = json!(row.succeeded);
                    variables["request_duration_ms"] = json!(row.request_elapsed);
                    variables["completion"] = json!(row.completion);
                    match jobs::collect(config, &item.workflow, &variables, directory, cancel).await
                    {
                        Ok((queue, execution)) => {
                            row.queue = Some(queue);
                            row.execution = Some(execution);
                            row.proof = true;
                        }
                        Err(_) => row.proof = false,
                    }
                }
            }
            let valid: Vec<_> = item
                .records
                .iter()
                .filter(|row| row.succeeded && row.proof)
                .collect();
            let name = item.workflow["name"]
                .as_str()
                .ok_or_else(|| Failure::new("invalid_workflow"))?;
            let durations = valid.iter().map(|row| row.elapsed).collect::<Vec<_>>();
            let queues = valid.iter().filter_map(|row| row.queue).collect::<Vec<_>>();
            let executions = valid
                .iter()
                .filter_map(|row| row.execution)
                .collect::<Vec<_>>();
            let failed = item.failed + item.records.len() as u64 - valid.len() as u64;
            self.scenarios.insert(
                name.into(),
                summarize(&durations, failed, item.elapsed, &queues, &executions),
            );
            let rows=item.records.iter().map(|row| {
                let mut document=json!({"scenario":name,"worker":row.variables["worker"],"cycle":row.variables["cycle"],
                    "business_duration_ms":row.elapsed,"request_duration_ms":row.request_elapsed,"succeeded":row.succeeded,"proof":row.proof});
                if let Some(failure)=&row.failure {document["failure"]=failure.clone();}
                if !row.proof {document["proof_failure"]=json!({"category":"job_timing_evidence_invalid"});}
                document
            }).collect::<Vec<_>>();
            artifacts::append(&directory.join("cycles.jsonl"), &rows)?;
            artifacts::append(&directory.join("session-events.jsonl"), &item.events)?;
            for (worker, requests) in item.http_requests.iter().enumerate() {
                let rows: Vec<_> = requests
                    .iter()
                    .enumerate()
                    .map(|(index, row)| {
                        json!({
                            "scenario": name,
                            "worker": worker,
                            "sequence": index + 1,
                            "operation": row.operation,
                            "status": row.status,
                            "duration_ms": row.duration_ms,
                            "request_id": row.request_id.map(|id| id.to_string()),
                        })
                    })
                    .collect();
                artifacts::append(&directory.join("http-requests.jsonl"), &rows)?;
            }
        }
        if !self.pending.is_empty() {
            self.completed_cycles = self
                .scenarios
                .values()
                .map(|row| row["completed"].as_u64().unwrap_or(0))
                .sum();
            self.failed_cycles = self
                .scenarios
                .values()
                .map(|row| row["failed"].as_u64().unwrap_or(0))
                .sum();
        }
        Ok(())
    }
}

pub(crate) async fn run(
    config: &Value,
    catalog: &BTreeMap<String, Value>,
    input: &Input,
    _directory: &Path,
    samples: Option<&Samples>,
    pacing: &Pacing,
    cancel: &Cancellation,
) -> NativeResult<Measurement> {
    let workflows = config["contract"]["workloads"][&input.suite]
        .as_array()
        .ok_or_else(|| Failure::new("invalid_workflow"))?;
    let waiter = if workflows.iter().any(|row| row.get("job").is_some()) {
        Some(jobs::Waiter::start(config, cancel).await?)
    } else {
        None
    };
    let mut result = Measurement {
        scenarios: BTreeMap::new(),
        network_requests: 0,
        completed_cycles: 0,
        failed_cycles: 0,
        session_failures: 0,
        pending: Vec::new(),
    };
    let measured = async {
        for workflow in workflows {
            let name = workflow["name"]
                .as_str()
                .ok_or_else(|| Failure::new("invalid_workflow"))?;
            let identities =
                configuration::identities(config, &input.suite, name, input.concurrency)?;
            let started = Instant::now();
            let workers = identities
                .into_iter()
                .enumerate()
                .map(|(worker, identity)| {
                    worker_run(
                        config,
                        catalog,
                        workflow,
                        identity,
                        worker,
                        samples,
                        pacing,
                        waiter.as_ref(),
                        cancel,
                    )
                });
            let outcomes = join_all(workers).await;
            let mut pending = Pending {
                workflow: workflow.clone(),
                records: Vec::new(),
                failed: 0,
                elapsed: super::milliseconds(started.elapsed()),
                events: Vec::new(),
                http_requests: Vec::new(),
            };
            for outcome in outcomes {
                let outcome = outcome?;
                pending.failed += outcome.failed;
                result.network_requests += outcome.requests;
                result.session_failures += outcome
                    .events
                    .iter()
                    .filter(|row| row["succeeded"] != true)
                    .count() as u64;
                pending.records.extend(outcome.records);
                pending.events.extend(outcome.events);
                pending.http_requests.push(outcome.http_requests);
            }
            result.pending.push(pending);
        }
        Ok(())
    }
    .await;
    if let Some(waiter) = waiter
        && waiter.close().await.is_err()
    {
        for item in result
            .pending
            .iter_mut()
            .filter(|item| item.workflow.get("job").is_some())
        {
            for row in &mut item.records {
                row.succeeded = false;
                row.failure = Some(json!({"category":"job_observer_failed"}));
            }
        }
    }
    measured?;
    Ok(result)
}

struct WorkerResult {
    records: Vec<Cycle>,
    events: Vec<Value>,
    failed: u64,
    requests: u64,
    http_requests: Vec<RequestObservation>,
}
#[allow(clippy::too_many_arguments)]
async fn worker_run(
    config: &Value,
    catalog: &BTreeMap<String, Value>,
    workflow: &Value,
    identity: Value,
    worker: usize,
    samples: Option<&Samples>,
    pacing: &Pacing,
    waiter: Option<&jobs::Waiter>,
    cancel: &Cancellation,
) -> NativeResult<WorkerResult> {
    let mut session = Session::new(config, catalog, identity.clone(), samples)?;
    let cycles = config["contract"]["cycles"]
        .as_u64()
        .ok_or_else(|| Failure::new("invalid_cycles"))?;
    let name = workflow["name"]
        .as_str()
        .ok_or_else(|| Failure::new("invalid_workflow"))?;
    let mut result = WorkerResult {
        records: Vec::new(),
        events: Vec::new(),
        failed: 0,
        requests: 0,
        http_requests: Vec::new(),
    };
    let started = Instant::now();
    let login = session.login(pacing, cancel).await;
    let mut event = json!({"scenario":name,"worker":worker,"phase":"login","duration_ms":super::milliseconds(started.elapsed()),"succeeded":login.is_ok()});
    if let Err(error) = login {
        event["failed_cycles"] = json!(cycles);
        event["failure"] = error.document();
        result.events.push(event);
        result.failed = cycles;
        result.requests = session.requests;
        result.http_requests = session.observations;
        return Ok(result);
    }
    result.events.push(event);
    for cycle in 0..cycles {
        let before = Instant::now();
        let mut variables = json!({"tenant_id":identity["tenant_id"],"username":identity["username"],"unique":format!("bench-{}",uuid::Uuid::new_v4()),"cycle":cycle,"worker":worker});
        let executed = session
            .workflow(workflow, &mut variables, pacing, cancel)
            .await;
        let request_elapsed = super::milliseconds(before.elapsed());
        let mut failure = executed.as_ref().err().map(Failure::document);
        let mut succeeded = executed.is_ok();
        let completion = if workflow.get("job").is_some() {
            let completion = match (waiter, jobs::selection(workflow, &variables)) {
                (Some(waiter), Ok(selection)) => waiter
                    .wait(
                        selection,
                        std::time::Duration::from_millis(
                            config["contract"]["timeout_ms"].as_u64().unwrap_or(0),
                        ),
                        cancel,
                    )
                    .await
                    .unwrap_or_else(
                        |_| json!({"valid":false,"reason":"job_completion_binding_failed"}),
                    ),
                _ => json!({"valid":false,"reason":"job_completion_binding_failed"}),
            };
            succeeded &= completion["valid"] == true;
            if completion["valid"] != true && failure.is_none() {
                failure = Some(json!({"category":"job_completion_failed"}));
            }
            Some(completion)
        } else {
            None
        };
        result.records.push(Cycle {
            variables,
            succeeded,
            request_elapsed,
            elapsed: super::milliseconds(before.elapsed()),
            completion,
            proof: workflow.get("job").is_none(),
            failure,
            queue: None,
            execution: None,
        });
    }
    let started = Instant::now();
    let logout = session
        .request(&json!({"operation":"post_auth_logout"}), pacing, cancel)
        .await;
    let mut event = json!({"scenario":name,"worker":worker,"phase":"logout","duration_ms":super::milliseconds(started.elapsed()),"succeeded":logout.is_ok()});
    if let Err(error) = logout {
        event["failure"] = error.document();
    }
    result.events.push(event);
    result.requests = session.requests;
    result.http_requests = session.observations;
    Ok(result)
}
