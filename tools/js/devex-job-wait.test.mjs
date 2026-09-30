import test from 'node:test'
import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import { PassThrough } from 'node:stream'
import path from 'node:path'
import { load } from './load.mjs'
import { jobWaiter } from './job-wait.mjs'
import { identityPool } from './devex-selection-fixture.mjs'

const workflow = {
  name: 'message',
  identity_pool: 'system',
  job: { kind: 'message', id: '${message_id}', job_type: 'system.message.dispatch' },
}
const pool = identityPool(2, { system: true })
const config = {
  contract: { cycles: 2, timeout_ms: 1000, workloads: { jobs: [workflow] }, identity_pools: { system: pool.contract } },
  bindings: {
    scope_id: 'job-wait-test',
    identity_pools: { system: pool.binding },
    job_timings: {
      python: path.resolve('python-fixture'),
      mysql_client: path.resolve('mysql-fixture'),
    },
  },
}

function deferred() {
  let resolve
  const promise = new Promise((callback) => {
    resolve = callback
  })
  return { promise, resolve }
}

test('任务周期等待真实完成后才发起下一周期，单观察器贯穿业务计时', async () => {
  const submitted = [],
    awaiting = [],
    rounds = [deferred(), deferred()]
  let created = 0,
    closed = 0
  class Session {
    requests = 0
    async login() {}
    async request() {}
    async workflow(_workflow, variables) {
      variables.message_id = String(100 + submitted.length)
      submitted.push({ ...variables })
    }
  }
  const measuring = load(config, {}, 'jobs', 2, 'unused', {
    Session,
    jobWaiter: async () => {
      created++
      return {
        wait(_workflow, variables) {
          const waiting = deferred()
          awaiting.push(waiting)
          if (awaiting.length % 2 === 0) rounds[variables.cycle].resolve()
          return waiting.promise
        },
        async close() {
          closed++
        },
      }
    },
  })
  await rounds[0].promise
  assert.equal(submitted.length, 2)
  assert.equal(closed, 0)
  awaiting.slice(0, 2).forEach((value) => value.resolve({ valid: true, status: 'succeeded' }))
  await rounds[1].promise
  assert.equal(submitted.length, 4)
  awaiting.slice(2).forEach((value) => value.resolve({ valid: false, reason: 'job_cancelled' }))
  const result = await measuring
  assert.equal(created, 1)
  assert.equal(closed, 1)
  // 完整attempt收据尚未采集，此时不能凭入队或等待结果宣告通过。
  assert.deepEqual(result.scenarios, {})
})

function observerChild(transform = (value) => value) {
  const child = new EventEmitter()
  child.stdin = new PassThrough()
  child.stdout = new PassThrough()
  let initialized = false
  child.stdin.on('data', (chunk) => {
    for (const line of chunk.toString().trim().split('\n')) {
      const request = JSON.parse(line)
      const result = initialized
        ? transform({
            format_version: 1,
            valid: true,
            ticket: request.ticket,
            scope_id: config.bindings.scope_id,
            selection: request.selection,
            status: 'succeeded',
            job_id: '501',
          })
        : { format_version: 1, ready: true, scope_id: config.bindings.scope_id }
      initialized = true
      queueMicrotask(() => child.stdout.write(JSON.stringify(result) + '\n'))
    }
  })
  child.stdin.on('finish', () =>
    queueMicrotask(() => {
      child.stdout.end()
      child.emit('close', 0)
    }),
  )
  return child
}

test('多个等待通过同一子进程且逐个绑定精确租户与任务ID', async () => {
  let calls = 0
  const child = observerChild()
  const waiter = await jobWaiter(config, () => {
    calls++
    return child
  })
  const results = await Promise.all(
    ['101', '102'].map((message_id) => waiter.wait(workflow, { tenant_id: 'system', message_id })),
  )
  assert.equal(calls, 1)
  assert.deepEqual(
    results.map((row) => row.selection.id),
    ['101', '102'],
  )
  assert.ok(results.every((row) => row.valid))
  await waiter.close()
})

test('错误scope导致全部待完成测量失败且关闭阶段不能掩盖观察器故障', async () => {
  const child = observerChild((value) => ({ ...value, scope_id: 'other-scope' }))
  const waiter = await jobWaiter(config, () => child)
  const result = await waiter.wait(workflow, { tenant_id: 'system', message_id: '101' })
  assert.equal(result.valid, false)
  assert.equal(result.reason, 'job_observer_invalid_response')
  await assert.rejects(waiter.close(), /job_observer_invalid_response/)
})
