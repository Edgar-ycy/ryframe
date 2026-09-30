import { randomUUID } from 'node:crypto'
import { Session } from './request.mjs'
import { collectJob } from './job-timing.mjs'
import { jobWaiter } from './job-wait.mjs'
import { failureCategory } from './failure.mjs'
import { cycleEvidence } from './cycle-evidence.mjs'
import { selectWorkloadIdentities, validateSuiteSelection } from './selection.mjs'

export function quantile(values, fraction) {
  if (!values.length) return 0
  const sorted = [...values].sort((a, b) => a - b)
  return sorted[Math.max(0, Math.ceil(sorted.length * fraction) - 1)]
}

export function summarize(durations, failed, elapsed, queues = [], executions = []) {
  return {
    completed: durations.length,
    failed,
    elapsed_ms: elapsed,
    p50_ms: quantile(durations, 0.5),
    p95_ms: quantile(durations, 0.95),
    p99_ms: quantile(durations, 0.99),
    throughput: elapsed > 0 ? durations.length / (elapsed / 1000) : 0,
    queue_p95_ms: queues.length ? quantile(queues, 0.95) : null,
    execution_p95_ms: executions.length ? quantile(executions, 0.95) : null,
  }
}

export async function load(config, catalog, suite, concurrency, artifacts, dependencies = {}) {
  validateSuiteSelection(config, suite, concurrency)
  const now = dependencies.now ?? (() => performance.now())
  const scenarios = {}
  const pending = []
  let network_requests = 0
  const SessionType = dependencies.Session ?? Session
  const createWaiter = dependencies.jobWaiter ?? jobWaiter
  const waiter = config.contract.workloads[suite].some((workflow) => workflow.job)
    ? await createWaiter(config)
    : undefined
  try {
    for (const workflow of config.contract.workloads[suite]) {
      const identities = selectWorkloadIdentities(config, suite, workflow.name, concurrency)
      const records = []
      const events = []
      let failed = 0
      const started = now()
      await Promise.all(
        Array.from({ length: concurrency }, async (_, worker) => {
          const identity = identities[worker]
          const session = new SessionType(config, catalog, identity, {
            ...(dependencies.pacing?.fixedControls(identity) ?? {}), multipartSample: dependencies.multipartSample,
          })
          const loginStarted = now()
          try {
            await session.login()
            events.push({
              scenario: workflow.name,
              worker,
              phase: 'login',
              duration_ms: now() - loginStarted,
              succeeded: true,
            })
          } catch (error) {
            events.push({
              scenario: workflow.name,
              worker,
              phase: 'login',
              duration_ms: now() - loginStarted,
              succeeded: false,
              failed_cycles: config.contract.cycles,
              failure: failureCategory(error),
            })
            failed += config.contract.cycles
            network_requests += session.requests
            return
          }
          for (let cycle = 0; cycle < config.contract.cycles; cycle++) {
            const before = now()
            const variables = {
              tenant_id: identity.tenant_id,
              username: identity.username,
              unique: `bench-${randomUUID()}`,
              cycle,
              worker,
            }
            let succeeded = false
            let failure
            try {
              await session.workflow(workflow, variables)
              succeeded = true
            } catch (error) {
              failure = failureCategory(error)
            }
            const requestElapsed = now() - before
            let completion
            if (workflow.job) {
              try {
                completion = await waiter.wait(workflow, variables)
              } catch {
                completion = { valid: false, reason: 'job_completion_binding_failed' }
              }
              succeeded &&= completion.valid
              if (!completion.valid) failure ??= { category: 'job_completion_failed' }
            }
            records.push({
              variables,
              succeeded,
              requestElapsed,
              completion,
              elapsed: now() - before,
              proof: !workflow.job,
              failure,
            })
          }
          const logoutStarted = now()
          try {
            await session.request({ operation: 'post_auth_logout' })
            events.push({
              scenario: workflow.name,
              worker,
              phase: 'logout',
              duration_ms: now() - logoutStarted,
              succeeded: true,
            })
          } catch (error) {
            events.push({
              scenario: workflow.name,
              worker,
              phase: 'logout',
              duration_ms: now() - logoutStarted,
              succeeded: false,
              failure: failureCategory(error),
            })
          }
          network_requests += session.requests
        }),
      )
      const elapsed = now() - started
      pending.push({ workflow, records, failed, elapsed, events })
    }
  } finally {
    try {
      await waiter?.close()
    } catch {
      for (const item of pending.filter((value) => value.workflow.job)) {
        for (const row of item.records) {
          row.succeeded = false
          row.failure = { category: 'job_observer_failed' }
        }
      }
    }
  }
  const measurement = {
    scenarios,
    network_requests,
    completed_cycles: 0,
    failed_cycles: 0,
    session_failures: pending.flatMap((item) => item.events).filter((event) => !event.succeeded).length,
    finalize: async () => {
      await completeMeasurements(pending, scenarios, config, artifacts)
      measurement.completed_cycles = Object.values(scenarios).reduce((total, row) => total + row.completed, 0)
      measurement.failed_cycles = Object.values(scenarios).reduce((total, row) => total + row.failed, 0)
    },
  }
  return measurement
}

export async function completeMeasurements(
  pending,
  scenarios,
  config,
  artifacts,
  collect = collectJob,
) {
  // 精确任务终态等待已包含在业务计时中；停止资源观察后才读取完整尝试历史。
  for (const { workflow, records, failed, elapsed, events } of pending) {
    if (workflow.job) {
      for (const row of records) {
        const variables = {
          ...row.variables,
          business_duration_ms: row.elapsed,
          business_succeeded: row.succeeded,
          request_duration_ms: row.requestElapsed,
          completion: row.completion,
        }
        try {
          const [queue, execution] = await collect(config, workflow, variables, artifacts)
          Object.assign(row, { queue, execution, proof: true })
        } catch {
          row.proof = false
        }
      }
    }
    const valid = records.filter((row) => row.succeeded && row.proof)
    scenarios[workflow.name] = summarize(
      valid.map((row) => row.elapsed),
      failed + records.length - valid.length,
      elapsed,
      valid.flatMap((row) => (row.queue === undefined ? [] : [row.queue])),
      valid.flatMap((row) => (row.execution === undefined ? [] : [row.execution])),
    )
    await cycleEvidence(artifacts, workflow, records, events)
  }
}
