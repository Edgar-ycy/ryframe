import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import {
  datasetArguments,
  referenceClient,
  verifyExisting,
} from '../restore_reference_existing.mjs'
import { Session } from '../devex/request.mjs'
import { hash } from '../devex/config.mjs'

const backend = fileURLToPath(new URL('../../', import.meta.url))
const bytes = Buffer.from('existing-object')

function fixture() {
  const plan = {
    source: {
      scope_id: 'original',
      api_url: 'http://127.0.0.1:3100',
      frontend_url: 'http://127.0.0.1:4100',
    },
    target: {
      scope_id: 'restored',
      api_url: 'http://127.0.0.1:3200',
      frontend_url: 'http://127.0.0.1:4200',
    },
    dataset: { request_interval_ms: 1000 },
  }
  const tenants = Array.from({ length: 11 }, (_, index) => ({
    tenant_id: index ? `original-${String(index).padStart(2, '0')}` : 'system',
    username: 'owner',
    password_env: 'TEST_PASSWORD',
    posts: [{ id: String(index + 1), code: `code-${index}`, name: `岗位${index}` }],
    files: [
      {
        file_path: `tenant-${index}/existing.txt`,
        bytes: bytes.length,
        sha256: createHash('sha256').update(bytes).digest('hex'),
      },
    ],
  }))
  return {
    plan,
    dataset: { format_version: 1, plan_sha256: hash(plan), source_scope_id: 'original', tenants },
  }
}

function transport(t, dataset, options = {}) {
  const calls = []
  t.mock.method(Session.prototype, 'login', async function () {
    calls.push({ operation: 'login', identity: this.identity, api: this.config.bindings.api_url })
    this.token = 'fixture-token'
    this.beforeRequest = async () => {}
  })
  t.mock.method(Session.prototype, 'request', async function (step) {
    calls.push({
      operation: step.operation,
      identity: this.identity,
      api: this.config.bindings.api_url,
    })
    if (step.operation === 'post_auth_logout') {
      if (options.logoutFailure) throw new Error('logout failed')
      return {}
    }
    assert.equal(step.operation, 'get_system_posts_by_id')
    const post = dataset.tenants.find((value) => value.tenant_id === this.identity.tenant_id)
      .posts[0]
    assert.equal(step.path.id, post.id)
    return { data: { ...post, ...(options.wrongPost ? { code: 'wrong' } : {}) } }
  })
  t.mock.method(globalThis, 'fetch', async (url, request) => {
    calls.push({ operation: 'download', url, request })
    if (options.downloadStatus) return new Response(null, { status: options.downloadStatus })
    if (options.oversized)
      return new Response(
        new ReadableStream({
          pull(controller) {
            controller.enqueue(bytes)
          },
          cancel() {
            options.onCancel()
          },
        }),
      )
    return new Response(options.wrongFile ? 'wrong' : bytes)
  })
  return calls
}

test('已有数据默认 target，显式 source；造数、缺值、重复及未知侧均失败关闭', () => {
  const base = ['--plan', 'plan.json', '--backend-dir', backend, '--write']
  assert.equal(datasetArguments(base).has('--side'), false)
  assert.equal(
    datasetArguments([...base, '--verify-existing', 'data.json']).get('--side'),
    'target',
  )
  assert.equal(
    datasetArguments([...base, '--verify-existing', 'data.json', '--side', 'source']).get('--side'),
    'source',
  )
  for (const args of [
    [...base, '--side', 'source'],
    [...base, '--verify-existing', 'data.json', '--side', 'other'],
    [...base, '--verify-existing'],
    [...base, '--write'],
    [...base, '--unknown', 'x'],
    base.filter((value) => value !== '--write'),
  ])
    assert.throws(() => datasetArguments(args))
})

for (const side of ['source', 'target']) {
  test(`${side} 所有岗位和对象请求使用准确侧，保留原 source 租户及不变收据`, async (t) => {
    const { plan, dataset } = fixture()
    const original = JSON.stringify({ plan, dataset })
    const calls = transport(t, dataset)
    const result = await verifyExisting(plan, backend, dataset, side)
    assert.deepEqual(result, {
      format_version: 1,
      status: 'existing_data_verified',
      side,
      scope_id: plan[side].scope_id,
      plan_sha256: hash(plan),
      source_scope_id: 'original',
      actions: { business: 'read_only', objects: 'read_only', session: 'login_logout' },
      restore_success: false,
      tenants: 11,
      posts: 11,
      files: 11,
    })
    assert.equal(JSON.stringify({ plan, dataset }), original)
    assert.equal(calls.length, 44)
    for (const call of calls) {
      if (call.operation === 'download') {
        assert.equal(call.url.origin, new URL(plan[side].api_url).origin)
        assert.equal(call.url.searchParams.get('bucket'), 'uploads')
        assert.equal(call.request.redirect, 'error')
        assert.equal(call.request.method, undefined)
        assert.match(
          call.request.headers['X-Forwarded-For'],
          new RegExp(`^198\\.${side === 'source' ? 18 : 19}\\.`),
        )
      } else {
        assert.equal(call.api, plan[side].api_url)
        assert.ok(['login', 'post_auth_logout', 'get_system_posts_by_id'].includes(call.operation))
      }
    }
  })
}

test('未传侧的业务请求仍只验证 target', async (t) => {
  const { plan, dataset } = fixture()
  const calls = transport(t, dataset)
  assert.equal((await verifyExisting(plan, backend, dataset)).side, 'target')
  assert.ok(calls.filter((value) => value.api).every((value) => value.api === plan.target.api_url))
})

test('错计划、错原始scope、重复和越界租户均在任何业务请求前拒绝', async (t) => {
  const original = fixture()
  const calls = transport(t, original.dataset)
  for (const change of [
    (value) => {
      value.dataset.plan_sha256 = '0'.repeat(64)
    },
    (value) => {
      value.dataset.source_scope_id = 'restored'
    },
    (value) => {
      value.dataset.tenants[1].tenant_id = 'system'
    },
    (value) => {
      value.dataset.tenants[1].tenant_id = 'other-01'
    },
    (value) => {
      value.dataset.tenants[1].files[0].sha256 = 'invalid'
    },
  ]) {
    const candidate = structuredClone(original)
    change(candidate)
    await assert.rejects(verifyExisting(candidate.plan, backend, candidate.dataset, 'source'))
  }
  await assert.rejects(verifyExisting(original.plan, backend, original.dataset, 'unknown'))
  assert.equal(calls.length, 0)
  const plan = structuredClone(original.plan)
  plan.source.api_url = 'https://example.com'
  assert.throws(() => referenceClient(plan, new Map(), original.dataset.tenants[0]))
})

test('对象流超过声明大小立即中止，不读取无界响应或产生成功收据', async (t) => {
  const { plan, dataset } = fixture()
  let cancelled = 0
  const calls = transport(t, dataset, {
    oversized: true,
    onCancel: () => {
      cancelled++
    },
  })
  await assert.rejects(verifyExisting(plan, backend, dataset, 'source'), /超过登记的声明大小/)
  assert.equal(cancelled, 1)
  assert.equal(calls.at(-1).operation, 'post_auth_logout')
  assert.equal(calls.filter((value) => value.operation === 'login').length, 1)
})

for (const [name, options] of [
  ['岗位内容不符', { wrongPost: true }],
  ['对象内容不符', { wrongFile: true }],
  ['对象请求失败', { downloadStatus: 403 }],
  ['注销失败', { logoutFailure: true }],
  ['业务和注销同时失败', { wrongPost: true, logoutFailure: true }],
]) {
  test(`${name} 不产生成功收据，保留失败并注销`, async (t) => {
    const { plan, dataset } = fixture()
    const calls = transport(t, dataset, options)
    await assert.rejects(verifyExisting(plan, backend, dataset, 'source'), (error) => {
      if (options.wrongPost && options.logoutFailure) assert.equal(error.errors.length, 2)
      return true
    })
    assert.equal(calls.filter((value) => value.operation === 'login').length, 1)
    assert.equal(calls.at(-1).operation, 'post_auth_logout')
  })
}
