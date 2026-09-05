import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import path from 'node:path'
import os from 'node:os'
import { hash } from '../devex/config.mjs'
import { createPacing } from '../devex/pacing.mjs'
import { fixedPacer, validatePacingAuthority } from '../devex/pacing-model.mjs'
import { Session } from '../devex/request.mjs'
import { load } from '../devex/load.mjs'
import { measure } from '../devex/measure.mjs'
import { identityPool } from './devex-selection-fixture.mjs'

const catalog = new Map([
  ['get_auth_csrf', { method: 'GET', path: '/api/v1/auth/csrf' }],
  ['post_auth_login', { method: 'POST', path: '/api/v1/auth/login' }],
  ['post_auth_refresh', { method: 'POST', path: '/api/v1/auth/refresh' }],
  ['post_auth_logout', { method: 'POST', path: '/api/v1/auth/logout' }],
  ['post_auth_password_reset_complete', { method: 'POST', path: '/api/v1/auth/password-reset/complete' }],
  ['get_system_posts', { method: 'GET', path: '/api/v1/system/posts' }],
])
const authority = {
  format_version: 1, global: { capacity: 100, window_ms: 60000 },
  user: { enabled: false, capacity: 500, window_ms: 60000 },
  api: { window_ms: 60000, rules: {
    'POST /api/v1/auth/login': 5, 'GET /api/v1/auth/csrf': 30,
    'POST /api/v1/auth/password-reset/complete': 3,
  } }, login: { capacity: 5, window_ms: 60000 },
}
const pacing = () => ({ authority_sha256: hash(authority), request_interval_ms: 1000,
  sample_prepare_wait_ms: 61250, operation_intervals_ms: {
    get_auth_csrf: 2250, post_auth_login: 12250, post_auth_password_reset_complete: 20250,
  } })
const identity = { tenant_id: 'system', username: 'reader', client_address: '198.18.10.1' }
const metadata = (name) => ({ operation: name, method: catalog.get(name).method, route: catalog.get(name).path })
function config(root) {
  const pool = identityPool(1)
  return { contract: { pacing: pacing(), cycles: 1, timeout_ms: 1000,
    identity_pools: { users: pool.contract }, homepage: { identity_pool: 'users' } },
  bindings: { identity_pools: { users: pool.binding }, scope_id: 'pacing-test',
    api_url: 'http://127.0.0.1:18080', frontend_url: 'http://127.0.0.1:4174',
    pacing: { python: process.execPath, login_budget_state: path.join(root, 'login-budget.json') } } }
}
async function directory(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), 'devex-pacing-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  return root
}
function virtualClock() {
  let current = 0
  return { now: () => current, sleep: async (milliseconds) => { current += milliseconds } }
}

test('节奏按真实全局、用户、CSRF、登录及其他接口规则校验，示例不是固定门槛', () => {
  assert.doesNotThrow(() => validatePacingAuthority(pacing(), authority, catalog))
  for (const change of [
    (value) => { value.request_interval_ms = 600 },
    (value) => { value.sample_prepare_wait_ms = 60000 },
    (value) => { value.operation_intervals_ms.get_auth_csrf = 2000 },
    (value) => { value.operation_intervals_ms.post_auth_login = 12000 },
    (value) => { delete value.operation_intervals_ms.post_auth_password_reset_complete },
    (value) => { value.operation_intervals_ms.unknown = 1000 },
  ]) {
    const value = pacing(); change(value)
    assert.throws(() => validatePacingAuthority(value, authority, catalog))
  }
  const changed = structuredClone(authority)
  changed.user = { enabled: true, capacity: 10, window_ms: 60000 }
  assert.throws(() => validatePacingAuthority(pacing(), changed, catalog), /用户限流/)
  const conservative = { ...pacing(), request_interval_ms: 6500 }
  assert.doesNotThrow(() => validatePacingAuthority(conservative, changed, catalog))
  changed.api.rules['/unmatched'] = 1
  assert.throws(() => validatePacingAuthority(conservative, changed, catalog), /明确关联/)
})

test('每次独立sample调用都有完整准备窗口，包括预热及ABBA相邻同侧', async (t) => {
  const root = await directory(t), clock = virtualClock()
  for (const name of ['warmup-base', 'warmup-candidate', 'base-1', 'candidate-1', 'candidate-2', 'base-2']) {
    const output = path.join(root, name + '.json'), before = clock.now()
    let observedAt
    const result = await measure(config(root), { suite: 'homepage', backend: root, frontend: root, output }, 'warm', {
      verifyProvenance: async () => async () => {},
      createPacing: (value, paths) => createPacing(value, paths, { ...clock, catalog, readAuthority: async () => authority }),
      prepareImportSamples: async () => {},
      observe: async () => { observedAt = clock.now(); return async () => ({ duration: clock.now() - observedAt }) },
      homepage: async () => { await clock.sleep(100); return { duration: 100 } },
    })
    assert.equal(observedAt - before, 61250)
    assert.equal(result.measurement.duration, 100)
    assert.equal(result.resources.duration, 100)
    const receipt = JSON.parse(await readFile(path.join(output + '.artifacts', 'sample-preparation.json')))
    assert.equal(receipt.duration_ms, 61250)
    assert.equal(receipt.success, true)
    assert.equal(receipt.reason, 'fixed_rate_limit_window')
    assert.ok(receipt.started_at && receipt.finished_at)
  }
})

test('authority摘要不匹配先落盘失败，不进入样本窗口或请求', async (t) => {
  const root = await directory(t), value = config(root)
  value.contract.pacing.authority_sha256 = '0'.repeat(64)
  await assert.rejects(createPacing(value, { backend: root, frontend: root, artifacts: root }, {
    catalog, readAuthority: async () => authority,
  }), /摘要不一致/)
  const receipt = JSON.parse(await readFile(path.join(root, 'pacing-authority.json')))
  assert.equal(receipt.success, false)
  assert.equal(receipt.actual_sha256, hash(authority))
})

test('嵌套CSRF及跨Session同客户共用固定时钟，登录principal也不能换地址重置', async () => {
  const clock = virtualClock(), events = []
  const controls = fixedPacer(pacing(), authority, catalog, { ...clock, onWait: (event) => events.push(event) })
  const first = controls(identity), second = controls(identity)
  await first(metadata('get_auth_csrf'))
  await first(metadata('post_auth_login'))
  await second(metadata('get_auth_csrf'))
  await second(metadata('post_auth_refresh'))
  await second(metadata('get_auth_csrf'))
  await second(metadata('post_auth_login'))
  assert.equal(clock.now(), 13250)
  assert.deepEqual(events.map((event) => event.duration_ms), [0, 1000, 1250, 1000, 1250, 8750])
  assert.throws(() => controls({ ...identity, client_address: '198.18.10.2' }), /同一身份/)
  assert.throws(() => controls({ ...identity, client_address: '198.18.256.1' }), /明确客户地址/)
  await assert.rejects(first({ ...metadata('post_auth_login'), method: 'GET' }), /精确契约事实/)
})

test('准备阶段复用共享principal/IP登录预算，429不重试且失败仍完成预约', async (t) => {
  const root = await directory(t), clock = virtualClock(), budgetEvents = [], received = []
  const context = await createPacing(config(root), { backend: root, frontend: root, artifacts: root }, {
    ...clock, catalog, readAuthority: async () => authority,
    loginBudget: (options) => {
      assert.equal(options.capacity, 5); assert.equal(options.windowMs, 60000)
      return { reserve: async (subject, address) => { budgetEvents.push([subject, address]); return 'reservation' },
        complete: async (reservation) => { budgetEvents.push(reservation) } }
    },
  })
  const original = globalThis.fetch
  t.after(() => { globalThis.fetch = original })
  globalThis.fetch = async (_url, options) => { received.push(options); return new Response(null, { status: 429 }) }
  const session = new Session(config(root), catalog, identity, await context.createPreparationControls(identity))
  await assert.rejects(session.request({ operation: 'post_auth_login', body: {} }), /429/)
  assert.equal(received.length, 1)
  assert.deepEqual(budgetEvents, [[{ tenantId: 'system', username: 'reader' }, '198.18.10.1'], 'reservation'])
  await context.close()
  const summary = JSON.parse(await readFile(path.join(root, 'pacing-summary.json')))
  assert.equal(summary.preparation.post_auth_login.requests, 1)
  assert.deepEqual(summary.measurement, {})
})

test('完成hook同时保留网络/HTTP失败与预约失败，metadata含当前契约事实', async (t) => {
  const root = await directory(t), original = globalThis.fetch
  t.after(() => { globalThis.fetch = original })
  globalThis.fetch = async () => new Response(null, { status: 429 })
  const seen = []
  const session = new Session(config(root), catalog, identity, { beforeRequest: async (metadata) => {
    seen.push(metadata); return async () => { throw new Error('budget completion failed') }
  } })
  await assert.rejects(session.request({ operation: 'get_auth_csrf' }), (error) => {
    assert.ok(error instanceof AggregateError)
    assert.match(error.errors[0].message, /429/)
    assert.match(error.errors[1].message, /budget completion/)
    return true
  })
  assert.deepEqual(seen, [{ operation: 'get_auth_csrf', method: 'GET', route: '/api/v1/auth/csrf' }])
})

test('10/50/100独立客户首批同时放行，不能靠跨客户串行降低并发', async () => {
  for (const count of [10, 50, 100]) {
    const clock = virtualClock(), controls = fixedPacer(pacing(), authority, catalog, clock)
    const pool = identityPool(count)
    let started = 0
    await Promise.all(pool.binding.map((value, index) => controls({ ...value,
      client_address: pool.contract.slots[index].client_address })(metadata('get_system_posts')).then(() => { started++ })))
    assert.equal(started, count)
    assert.equal(clock.now(), 0)
  }
})

test('测量阶段固定间隔进入周期耗时与吞吐，准备窗口不进入分位数', async (t) => {
  const root = await directory(t), value = config(root), clock = virtualClock()
  value.contract.cycles = 2
  value.contract.workloads = { api: [{ name: 'list', identity_pool: 'users' }] }
  const fast = structuredClone(authority)
  fast.global.window_ms = 10; fast.api.window_ms = 10; fast.login.window_ms = 10
  value.contract.pacing = { authority_sha256: hash(fast), request_interval_ms: 15,
    operation_intervals_ms: {}, sample_prepare_wait_ms: 300 }
  const context = await createPacing(value, { backend: root, frontend: root, artifacts: root }, {
    ...clock, catalog, readAuthority: async () => fast,
  })
  class Client {
    requests = 0
    constructor(_config, _catalog, _identity, controls) { this.controls = controls }
    async login() { await this.controls.beforeRequest(metadata('get_system_posts')) }
    async request() {}
    async workflow() { await this.controls.beforeRequest(metadata('get_system_posts')); this.requests++ }
  }
  const result = await load(value, catalog, 'api', 1, root, { Session: Client, pacing: context, now: clock.now })
  await result.finalize()
  assert.equal(result.scenarios.list.p50_ms, 15)
  assert.equal(result.scenarios.list.elapsed_ms, 30)
  assert.equal(result.scenarios.list.throughput, 2 / .03)
  assert.equal(result.completed_cycles, 2)
  await context.close()
})
