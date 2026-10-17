import { spawn } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { hash } from './config.mjs'
import { operationCatalog } from './request.mjs'
import { fixedPacer, validatePacingAuthority } from './pacing-model.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
const encodingEnvironmentNames = new Set(['PYTHONUTF8', 'PYTHONIOENCODING'])

export function rateLimitInvocation(cargo = process.env.CARGO || 'cargo') {
  return Object.freeze({
    cargo,
    arguments: Object.freeze([
      'run', '--quiet', '--locked', '-p', 'xtask', '--', '__rate-limit', 'authority',
    ]),
    cwd: root,
  })
}

export function rateLimitEnvironment(environment = process.env) {
  return Object.fromEntries(Object.entries(environment).filter(([name]) => {
    const upper = name.toUpperCase()
    return !encodingEnvironmentNames.has(upper) && !upper.startsWith('APP_')
  }))
}

function requestEnvironment(environment = process.env) {
  return Object.fromEntries(
    Object.entries(environment).filter(([, value]) => typeof value === 'string'),
  )
}

function readAuthority(backend) {
  return new Promise((resolve, reject) => {
    const invocation = rateLimitInvocation()
    const child = spawn(invocation.cargo, invocation.arguments, {
      cwd: invocation.cwd, env: rateLimitEnvironment(), windowsHide: true, stdio: ['pipe', 'pipe', 'ignore'],
    })
    const chunks = []
    let bytes = 0
    let failure
    const fail = () => {
      failure = '无法从实际 APP 配置取得限流 authority'
      reject(new Error(failure))
    }
    const timer = setTimeout(() => { fail(); child.kill() }, 10000)
    child.on('error', () => { clearTimeout(timer); fail() })
    child.stdout.on('data', (chunk) => {
      bytes += chunk.length
      if (bytes > 64 * 1024) { clearTimeout(timer); fail(); child.kill() }
      else chunks.push(chunk)
    })
    child.stdin.on('error', () => {
      if (!failure) { clearTimeout(timer); fail(); child.kill() }
    })
    child.on('close', (code) => {
      clearTimeout(timer)
      if (failure || code !== 0) { if (!failure) fail(); return }
      try {
        const result = JSON.parse(Buffer.concat(chunks).toString('utf8'))
        if (!result || Object.keys(result).sort().join() !== 'ok,result' || result.ok !== true ||
            !result.result || typeof result.result !== 'object' || Array.isArray(result.result)) throw new Error()
        resolve(result.result)
      } catch { fail() }
    })
    child.stdin.end(JSON.stringify({ backend: path.resolve(backend), environment: requestEnvironment() }))
  })
}

export async function createPacing(config, { backend, runner_frontend, artifacts }, dependencies = {}) {
  const pacing = config.contract.pacing
  const binding = config.bindings.pacing
  const now = dependencies.now ?? (() => performance.now())
  const sleep = dependencies.sleep ?? delay
  await mkdir(artifacts, { recursive: true })
  let authority, catalog, failure
  const started = now()
  try {
    if (!binding || Object.keys(binding).sort().join() !== 'login_budget_state,python' ||
        !path.isAbsolute(binding.python) || !path.isAbsolute(binding.login_budget_state))
      throw new Error('固定节奏必须显式绑定 Python 和共享登录预算账本')
    authority = await (dependencies.readAuthority ?? readAuthority)(backend)
    catalog = dependencies.catalog ?? await operationCatalog(backend)
    validatePacingAuthority(pacing, authority, catalog)
    if (hash(authority) !== pacing.authority_sha256) throw new Error('限流 authority 与固定负载摘要不一致')
  } catch (error) { failure = error.message; throw error }
  finally {
    await writeFile(path.join(artifacts, 'pacing-authority.json'), JSON.stringify({
      format_version: 1, success: !failure, failure, duration_ms: now() - started, authority,
      actual_sha256: authority ? hash(authority) : undefined, expected_sha256: pacing?.authority_sha256,
    }, null, 2) + '\n', { flag: 'wx' })
  }
  return pacingContext(config, { runner_frontend, artifacts }, authority, catalog, { now, sleep, ...dependencies })
}

function pacingContext(config, { runner_frontend, artifacts }, authority, catalog, { now, sleep, loginBudget }) {
  const summary = { measurement: {}, preparation: {}, login_budget: { waits: 0, requested_ms: 0 } }
  const fixed = fixedPacer(config.contract.pacing, authority, catalog, { now, sleep,
    onWait: ({ phase, operation, requested_ms, duration_ms }) => {
      const row = summary[phase][operation] ??= { requests: 0, requested_ms: 0, duration_ms: 0 }
      row.requests++; row.requested_ms += requested_ms; row.duration_ms += duration_ms
    } })
  let budget
  const preparationBudget = async () => {
    if (!budget) {
      budget = (async () => {
        const create = loginBudget ?? (await import(pathToFileURL(
          path.join(runner_frontend, 'scripts/browser-login-budget.mjs')).href)).createLoginBudget
        return create({ statePath: config.bindings.pacing.login_budget_state, scope: config.bindings.scope_id,
          capacity: authority.login.capacity, windowMs: authority.login.window_ms,
          onWait: (milliseconds) => { summary.login_budget.waits++; summary.login_budget.requested_ms += milliseconds } })
      })()
    }
    return budget
  }
  return {
    fixedControls: (identity) => ({ beforeRequest: fixed(identity) }),
    async createPreparationControls(identity) {
      const before = fixed(identity, 'preparation')
      const shared = await preparationBudget()
      return { beforeRequest: (metadata) => before(metadata, async () => {
        if (metadata.operation !== 'post_auth_login') return undefined
        const reservation = await shared.reserve({ tenantId: identity.tenant_id, username: identity.username }, identity.client_address)
        return async () => { await shared.complete(reservation) }
      }) }
    },
    async prepareSample() {
      const started_at = new Date().toISOString(), before = now()
      const requested_ms = config.contract.pacing.sample_prepare_wait_ms
      let failure
      try {
        while (now() < before + requested_ms) {
          const previous = now()
          await sleep(Math.max(1, Math.ceil(before + requested_ms - previous)))
          if (now() < previous) throw new Error('样本准备等待期间时钟回退')
        }
      } catch (error) { failure = error.message; throw error }
      finally {
        await writeFile(path.join(artifacts, 'sample-preparation.json'), JSON.stringify({
          format_version: 1, reason: 'fixed_rate_limit_window', started_at,
          finished_at: new Date().toISOString(), requested_ms, duration_ms: now() - before,
          authority_sha256: config.contract.pacing.authority_sha256, success: !failure, failure,
        }, null, 2) + '\n', { flag: 'wx' })
      }
    },
    async close() {
      await writeFile(path.join(artifacts, 'pacing-summary.json'), JSON.stringify({
        format_version: 1, pacing: config.contract.pacing, ...summary,
      }, null, 2) + '\n', { flag: 'wx' })
    },
  }
}
