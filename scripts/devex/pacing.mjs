import { execFile } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { promisify } from 'node:util'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { hash } from './config.mjs'
import { operationCatalog } from './request.mjs'
import { fixedPacer, validatePacingAuthority } from './pacing-model.mjs'

const authorityScript = fileURLToPath(new URL('../full_stack_rate_limit_config.py', import.meta.url))

async function readAuthority(python, backend) {
  try {
    const result = await promisify(execFile)(python, ['-X', 'utf8', authorityScript, '--authority'], {
      cwd: backend, env: process.env, windowsHide: true, timeout: 10000, maxBuffer: 64 * 1024, encoding: 'utf8',
    })
    return JSON.parse(result.stdout)
  } catch { throw new Error('无法从实际 APP 配置取得限流 authority') }
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
    authority = await (dependencies.readAuthority ?? readAuthority)(binding.python, backend)
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
