import { spawn } from 'node:child_process'
import { appendFile, mkdir } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { setTimeout as delay } from 'node:timers/promises'
import { expand } from './config.mjs'

const collector = fileURLToPath(new URL('../devex_job_timings.py', import.meta.url))

export function jobSelection(workflow, variables) {
  const job = expand(workflow.job, variables)
  if (
    !job ||
    !['export', 'import', 'message', 'schedule'].includes(job.kind) ||
    typeof job.id !== 'string' ||
    !/^[1-9][0-9]{0,18}$/.test(job.id) ||
    BigInt(job.id) > 9223372036854775807n ||
    typeof job.job_type !== 'string' ||
    !/^[a-z][a-z0-9_.-]{0,95}$/.test(job.job_type)
  ) {
    throw new Error('任务时间采集需要明确业务ID、类型和关联种类')
  }
  return { kind: job.kind, id: job.id, job_type: job.job_type, tenant_id: variables.tenant_id }
}

function python(binding, request, timeout) {
  if (!binding || !path.isAbsolute(binding.python)) throw new Error('缺少显式 Python 采集器路径')
  return new Promise((resolve, reject) => {
    const child = spawn(binding.python, ['-X', 'utf8', collector], {
      stdio: ['pipe', 'pipe', 'ignore'],
      windowsHide: true,
      env: process.env,
    })
    const chunks = []
    let bytes = 0
    let failure
    const timer = setTimeout(() => {
      failure = 'collector_timeout'
      child.kill()
    }, timeout)
    child.on('error', () => {
      clearTimeout(timer)
      reject(new Error('collector_spawn_failed'))
    })
    child.stdout.on('data', (chunk) => {
      bytes += chunk.length
      if (bytes > 4 * 1024 * 1024) {
        failure = 'collector_output_limit'
        child.kill()
      } else chunks.push(chunk)
    })
    child.stdin.on('error', () => {
      failure ??= 'collector_input_failed'
    })
    child.on('close', (code) => {
      clearTimeout(timer)
      if (failure) {
        reject(new Error(failure))
        return
      }
      try {
        const value = JSON.parse(Buffer.concat(chunks).toString('utf8'))
        if (
          ![0, 1].includes(code) ||
          value.format_version !== 1 ||
          typeof value.valid !== 'boolean' ||
          (code === 0) !== value.valid
        )
          throw new Error('invalid')
        resolve(value)
      } catch {
        reject(new Error('collector_invalid_response'))
      }
    })
    child.stdin.end(JSON.stringify(request))
  })
}

export async function collectJob(config, workflow, variables, artifacts, run = python) {
  const started = performance.now()
  let selection
  let result
  try {
    selection = jobSelection(workflow, variables)
    const { python: executable, ...binding } = config.bindings.job_timings || {}
    const request = { binding, scope_id: config.bindings.scope_id, selection }
    do {
      result = await run(
        { python: executable },
        request,
        Math.min(config.contract.timeout_ms, 60000),
      )
      if (!['missing_job', 'message_not_dispatched', 'job_unfinished'].includes(result.reason))
        break
      if (performance.now() - started >= config.contract.timeout_ms) break
      await delay(250)
    } while (true)
    if (
      result.valid &&
      (result.scope_id !== request.scope_id ||
        result.kind !== selection.kind ||
        result.business_id !== selection.id ||
        !Number.isFinite(result.queue_ms) ||
        !Number.isFinite(result.execution_ms) ||
        result.queue_ms < 0 ||
        result.execution_ms < 0)
    ) {
      result = { valid: false, reason: 'collector_binding_mismatch' }
    }
  } catch {
    result = { valid: false, reason: 'collector_execution_failed' }
  }
  await mkdir(artifacts, { recursive: true })
  await appendFile(
    path.join(artifacts, 'job-attempts.jsonl'),
    JSON.stringify({
      workflow: workflow.name,
      worker: variables.worker,
      cycle: variables.cycle,
      business_duration_ms: variables.business_duration_ms,
      business_succeeded: variables.business_succeeded,
      request_duration_ms: variables.request_duration_ms,
      completion: variables.completion,
      collection_duration_ms: performance.now() - started,
      selection,
      ...result,
    }) + '\n',
  )
  if (!result.valid) throw new Error(`任务时间证据无效：${result.reason}`)
  return [result.queue_ms, result.execution_ms]
}
