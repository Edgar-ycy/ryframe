import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises'
import path from 'node:path'
import { load } from '../devex/load.mjs'
import { RequestFailure, failureCategory } from '../devex/failure.mjs'
import { identityPool, selectionConfig } from './devex-selection-fixture.mjs'

test('全部API周期与登录退出故障保留计数、耗时、HTTP分类且不包含私有变量', async (t) => {
  const root = path.resolve('.local-tests')
  await mkdir(root, { recursive: true })
  const directory = await mkdtemp(path.join(root, 'cycle-evidence-'))
  t.after(async () => {
    assert.equal(path.dirname(directory), root)
    await rm(directory, { recursive: true })
  })
  const pool = identityPool(3)
  pool.binding.forEach((identity, index) => {
    identity.tenant_id = 'private-tenant'
    identity.username = ['login-failure', 'mutation-failure', 'success'][index]
  })
  const config = selectionConfig(pool, 'api', 'write')
  config.contract.cycles = 2
  class Session {
    requests = 0
    constructor(_config, _catalog, identity) {
      this.identity = identity
    }
    async login() {
      if (this.identity.username === 'login-failure')
        throw new RequestFailure('http_status', 'post_auth_login', 429)
    }
    async workflow(_workflow, variables) {
      variables.token = 'secret-token-must-not-escape'
      if (this.identity.username === 'mutation-failure' && variables.cycle === 0) {
        throw new RequestFailure('http_status', 'post_system_posts', 409)
      }
    }
    async request() {
      if (this.identity.username === 'mutation-failure') throw new Error('private-response-body')
    }
  }
  const measurement = await load(config, {}, 'api', 3, directory, { Session })
  await measurement.finalize()
  assert.equal(measurement.scenarios.write.completed, 3)
  assert.equal(measurement.scenarios.write.failed, 3)
  assert.equal(measurement.completed_cycles, 3)
  assert.equal(measurement.failed_cycles, 3)
  assert.equal(measurement.session_failures, 2)
  const raw = await readFile(path.join(directory, 'cycles.jsonl'), 'utf8')
  const eventsRaw = await readFile(path.join(directory, 'session-events.jsonl'), 'utf8')
  for (const secret of ['secret-token', 'private-response', 'private-tenant', 'mutation-failure']) {
    assert.ok(!raw.includes(secret) && !eventsRaw.includes(secret))
  }
  const rows = raw.trim().split('\n').map(JSON.parse)
  assert.equal(rows.length, 4)
  assert.ok(
    rows.every((row) => row.business_duration_ms >= 0 && row.request_duration_ms >= 0 && row.proof),
  )
  assert.deepEqual(rows.find((row) => !row.succeeded).failure, {
    category: 'http_status',
    operation: 'post_system_posts',
    status: 409,
  })
  const events = eventsRaw.trim().split('\n').map(JSON.parse)
  assert.equal(events.length, 5)
  assert.equal(events.find((row) => row.phase === 'login' && !row.succeeded).failed_cycles, 2)
  assert.deepEqual(events.find((row) => row.phase === 'logout' && !row.succeeded).failure, {
    category: 'request_failed',
  })
})

test('失败分类只接受显式错误类型，不复制任意错误消息或伪造字段', () => {
  assert.deepEqual(failureCategory(new Error('database-password')), { category: 'request_failed' })
  assert.deepEqual(failureCategory({ category: 'http_status', operation: 'secret', status: 409 }), {
    category: 'request_failed',
  })
  assert.deepEqual(failureCategory({ name: 'TimeoutError' }), { category: 'request_timeout' })
  assert.deepEqual(failureCategory(new RequestFailure('poll_failed', 'get_job')), {
    category: 'poll_failed',
    operation: 'get_job',
  })
})
