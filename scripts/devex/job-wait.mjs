import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { jobSelection } from './job-timing.mjs'

const script = fileURLToPath(new URL('../devex_job_wait.py', import.meta.url))

export async function jobWaiter(config, launch = spawn) {
  const { python, ...binding } = config.bindings.job_timings
  const child = launch(python, ['-X', 'utf8', script], {
    stdio: ['pipe', 'pipe', 'ignore'],
    windowsHide: true,
    env: process.env,
  })
  const pending = new Map()
  let ticket = 0,
    failed,
    readyResolve,
    readyReject,
    initialized = false,
    closing = false
  const ready = new Promise((resolve, reject) => {
    readyResolve = resolve
    readyReject = reject
  })
  const fail = (reason) => {
    failed ??= reason
    readyReject(new Error(failed))
    for (const { resolve, timer } of pending.values()) {
      clearTimeout(timer)
      resolve({ valid: false, reason: failed })
    }
    pending.clear()
    child.stdin.end()
  }
  const startTimer = setTimeout(() => fail('job_observer_start_timeout'), 15000)
  child.on('error', () => fail('job_observer_start_failed'))
  child.stdin.on('error', () => fail('job_observer_input_failed'))
  const lines = createInterface({ input: child.stdout })
  lines.on('line', (line) => {
    try {
      if (line.length > 32768) throw new Error('limit')
      const result = JSON.parse(line)
      if (result.format_version !== 1) throw new Error('format')
      if (result.ready === true && result.scope_id === config.bindings.scope_id) {
        clearTimeout(startTimer)
        initialized = true
        readyResolve()
        return
      }
      const item = pending.get(result.ticket)
      if (
        !item ||
        result.scope_id !== config.bindings.scope_id ||
        JSON.stringify(result.selection) !== JSON.stringify(item.selection) ||
        typeof result.valid !== 'boolean'
      )
        throw new Error('binding')
      clearTimeout(item.timer)
      pending.delete(result.ticket)
      item.resolve(result)
    } catch {
      fail('job_observer_invalid_response')
    }
  })
  const closed = new Promise((resolve) =>
    child.on('close', (code) => {
      clearTimeout(startTimer)
      if (code !== 0 || pending.size || !initialized || !closing) fail('job_observer_exit_failed')
      resolve()
    }),
  )
  child.stdin.write(JSON.stringify({ binding, scope_id: config.bindings.scope_id }) + '\n')
  try {
    await ready
  } catch (error) {
    await closed
    throw error
  }
  return {
    wait(workflow, variables) {
      if (failed) return Promise.resolve({ valid: false, reason: failed })
      const selection = jobSelection(workflow, variables)
      const id = ++ticket
      return new Promise((resolve) => {
        const timer = setTimeout(
          () => fail('job_observer_response_timeout'),
          config.contract.timeout_ms + 15000,
        )
        pending.set(id, { selection, resolve, timer })
        child.stdin.write(
          JSON.stringify({ ticket: id, selection, timeout_ms: config.contract.timeout_ms }) + '\n',
        )
      })
    },
    async close() {
      closing = true
      child.stdin.end()
      await closed
      lines.close()
      if (failed) throw new Error(failed)
    },
  }
}
