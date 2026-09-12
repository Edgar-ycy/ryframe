import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import {
  datasetSpecification,
  privateDatasetArguments,
  postBatchRows,
  postBatchSamples,
  sampleContent,
  seedPosts,
  verifyDatasetPreflight,
} from '../restore_reference_dataset.mjs'
import { Session } from '../devex/request.mjs'
import { hash } from '../devex/config.mjs'
import { requestPacer } from '../restore_reference_pacing.mjs'

function protocolEnvironment(value, extra = {}) {
  return {
    RYFRAME_XTASK_RECOVERY_DATASET_PREPARE:
      typeof value === 'string' ? value : JSON.stringify(value),
    ...extra,
  }
}

function plan() {
  return {
    id: 'restore-case',
    source: {
      api_url: 'http://127.0.0.1:3000',
      frontend_url: 'http://127.0.0.1:4174',
      databases: [
        { key: 'control', mode: 'shared' },
        { key: 'dedicated', mode: 'dedicated' },
      ],
    },
    dataset: {
      request_interval_ms: 1000,
      api_validation_posts: 2,
      post_batch_rows: 1000,
      tenant_targets: [...Array(9).fill('control'), 'dedicated'],
      records: 100_000,
      object_count: 256,
      object_bytes: 4 * 1024 * 1024,
      admin: { tenant_id: 'system', username: 'admin', password_env: 'TEST_ADMIN' },
      owner_password_env: 'TEST_OWNER',
    },
  }
}

test('数据准备严格要求十个普通租户、共享与独立目标、十万记录与1GiB', () => {
  assert.equal(datasetSpecification(plan()).records, 100_000)
  for (const change of [
    (value) => {
      value.dataset.tenant_targets.pop()
    },
    (value) => {
      value.dataset.tenant_targets = Array(10).fill('control')
    },
    (value) => {
      value.dataset.tenant_targets[0] = 'dedicated'
    },
    (value) => {
      value.dataset.records = 99_999
    },
    (value) => {
      value.dataset.object_count = 255
    },
    (value) => {
      value.dataset.admin.tenant_id = 'other'
    },
    (value) => {
      value.source.api_url = 'https://example.com'
    },
    (value) => {
      delete value.dataset.request_interval_ms
    },
    (value) => {
      value.dataset.request_interval_ms = 100
    },
    (value) => {
      value.dataset.api_validation_posts = 0
    },
    (value) => {
      value.dataset.post_batch_rows = 99
    },
  ]) {
    const candidate = plan()
    change(candidate)
    assert.throws(() => datasetSpecification(candidate))
  }
})

test('岗位批次从每租户 API 验证记录后的确定索引开始，并只接受受控结果范围', () => {
  const candidate = plan()
  candidate.source.scope_id = 'source-scope'
  const identities = [
    { tenant_id: 'system' },
    ...Array.from({ length: 10 }, (_, index) => ({
      tenant_id: `source-scope-${String(index + 1).padStart(2, '0')}`,
    })),
  ]
  const batch = postBatchRows(candidate, identities)
  assert.equal(batch.rows.length, 100_000 - 22)
  assert.deepEqual(batch.rows[0], {
    tenant_id: 'system',
    index: 2,
    code: 'restore-case-2',
    name: '恢复样本2',
    sort: 2,
  })
  const samples = postBatchSamples(batch.samples, {
    kind: 'restore-reference-post-batch',
    first_id: '9007199254740000',
    last_id: '9007199254839977',
    rows: batch.rows.length,
    batch_rows: 1000,
    batches: 100,
  })
  assert.equal(samples[0].id, '9007199254740000')
  assert.equal(samples.at(-1).code, 'restore-case-9089')
  assert.throws(() => postBatchSamples(batch.samples, { first_id: '1' }))
})

test('对象样本大小固定、可重现且不同对象使用不同内容', () => {
  const sample = sampleContent('case', 0, 1024)
  assert.equal(sample.length, 1024)
  assert.deepEqual(sample, sampleContent('case', 0, 1024))
  assert.notDeepEqual(sample, sampleContent('case', 1, 1024))
  assert.match(sample.toString(), /^[a-f0-9\n]+$/)
})

test('数据准备只接受当前 source 空源预检收据', () => {
  const candidate = plan()
  candidate.source.scope_id = 'source-scope'
  const preflight = {
    format_version: 1,
    kind: 'restore-reference-dataset-preflight',
    plan_sha256: hash(candidate),
    side: 'source',
    scope_id: 'source-scope',
    actions: { business: 'empty_source_verified' },
  }
  verifyDatasetPreflight(candidate, preflight)
  assert.throws(() => verifyDatasetPreflight(candidate, { ...preflight, side: 'target' }))
  assert.throws(() =>
    verifyDatasetPreflight(candidate, { ...preflight, actions: { business: 'unchecked' } }),
  )
})

test('每条实际岗位创建与最终精确筛选计数均形成数据收据', async () => {
  const calls = [],
    created = []
  const session = {
    async request(step) {
      calls.push(step)
      if (step.operation === 'post_system_posts')
        return { data: { ...step.body, id: String(calls.length) } }
      return { data: { total: 3 } }
    },
  }
  const samples = await seedPosts(session, 'case', 'system', 3, async (value) => {
    created.push(value)
  })
  assert.equal(samples.length, 3)
  assert.equal(created.length, 3)
  assert.equal(calls.length, 4)
  assert.deepEqual(calls.at(-1).query, { code: 'case', page: 1, page_size: 1 })
  for (const response of [{ data: { id: '1', code: 'wrong' } }, { data: { code: 'case-0' } }])
    await assert.rejects(
      seedPosts({ request: async () => response }, 'case', 'system', 3, async () => {}),
    )
  await assert.rejects(
    seedPosts(
      {
        request: async (step) =>
          step.body ? { data: { ...step.body, id: '1' } } : { data: { total: 99 } },
      },
      'case',
      'system',
      3,
      async () => {},
    ),
  )
})

test('超过一千条的参考岗位仍满足当前契约且编码和名称保持唯一', async () => {
  const openapi = JSON.parse(await readFile(new URL('../../openapi/openapi.json', import.meta.url)))
  const post = openapi['x-ryframe-crud-resources'].resources.find(
    (resource) => resource.api.operations.create === 'post_system_posts',
  )
  const range = post.fields.find((field) => field.name === 'sort').validation
  assert.equal(range.minimum, 0)
  assert.equal(range.maximum, 999)
  const bodies = [],
    records = []
  let refreshes = 0
  const count = 1001
  const samples = await seedPosts(
    {
      async request(step) {
        if (step.operation === 'post_auth_refresh') {
          refreshes++
          return {}
        }
        if (step.operation === 'get_system_posts') return { data: { total: count } }
        assert.equal(step.operation, 'post_system_posts')
        assert.ok(step.body.sort >= range.minimum && step.body.sort <= range.maximum)
        bodies.push(step.body)
        return { data: { id: String(bodies.length), ...step.body } }
      },
    },
    'restore-boundary',
    'system',
    count,
    async (record) => records.push(record),
  )
  assert.equal(records.length, count)
  assert.equal(new Set(bodies.map((body) => body.code)).size, count)
  assert.equal(new Set(bodies.map((body) => body.name)).size, count)
  assert.equal(refreshes, 10)
  assert.equal(samples.at(-1).code, 'restore-boundary-1000')
})

test('明确本机参考客户才可发送隔离地址，普通 Session 不增加转发头', async () => {
  const config = {
    bindings: { api_url: 'http://127.0.0.1:3000', frontend_url: 'http://127.0.0.1:4174' },
    contract: { timeout_ms: 1000 },
  }
  const catalog = new Map([['get_probe', { method: 'GET', path: '/probe' }]])
  const identity = { tenant_id: 'system', username: 'admin' }
  for (const address of ['127.0.0.1', '198.18.0.256', '198.18.0.1, 1.2.3.4'])
    assert.throws(() => new Session(config, catalog, { ...identity, client_address: address }))
  assert.throws(
    () =>
      new Session(
        { ...config, bindings: { ...config.bindings, api_url: 'https://example.com' } },
        catalog,
        { ...identity, client_address: '198.18.20.1' },
      ),
  )
  const original = globalThis.fetch
  const received = []
  globalThis.fetch = async (_url, request) => {
    received.push(request.headers)
    return new Response(JSON.stringify({ code: 200 }), {
      headers: { 'content-type': 'application/json' },
    })
  }
  try {
    await new Session(config, catalog, identity).request({ operation: 'get_probe' })
    await new Session(config, catalog, { ...identity, client_address: '198.18.20.1' }).request({
      operation: 'get_probe',
    })
    assert.equal(received[0]['X-Forwarded-For'], undefined)
    assert.equal(received[1]['X-Forwarded-For'], '198.18.20.1')
  } finally {
    globalThis.fetch = original
  }
})

test('固定节奏包含刷新嵌套CSRF的每个真实HTTP请求，429不自动重试', async () => {
  let now = 0
  const starts = [],
    waits = []
  const beforeRequest = requestPacer(
    1000,
    () => now,
    async (milliseconds) => {
      waits.push(milliseconds)
      now += milliseconds
    },
  )
  const catalog = new Map(
    ['get_auth_csrf', 'post_auth_refresh', 'get_probe'].map((key) => [
      key,
      { method: key.startsWith('get') ? 'GET' : 'POST', path: '/' + key },
    ]),
  )
  const config = {
    bindings: { api_url: 'http://127.0.0.1:3000', frontend_url: 'http://127.0.0.1:4174' },
    contract: { timeout_ms: 1000 },
  }
  const session = new Session(config, catalog, { tenant_id: 'system' }, { beforeRequest })
  const original = globalThis.fetch
  globalThis.fetch = async () => {
    starts.push(now)
    return new Response(JSON.stringify({ code: 200, data: { access_token: 'fixture' } }), {
      headers: { 'content-type': 'application/json' },
    })
  }
  try {
    await session.request({ operation: 'post_auth_refresh' })
    await session.request({ operation: 'get_probe' })
    assert.deepEqual(starts, [0, 1000, 2000])
    assert.deepEqual(waits, [1000, 1000])
    let rejected = 0
    globalThis.fetch = async () => {
      rejected++
      return new Response(null, { status: 429 })
    }
    await assert.rejects(session.request({ operation: 'get_probe' }), /HTTP 429/)
    assert.equal(rejected, 1)
  } finally {
    globalThis.fetch = original
  }
})

test('参考创建每100条刷新且非法节奏不允许启动', async () => {
  for (const interval of [undefined, 999, 5001, 1000.5]) assert.throws(() => requestPacer(interval))
  const calls = []
  await seedPosts(
    {
      async request(step) {
        calls.push(step.operation)
        return step.body
          ? { data: { id: String(calls.length), ...step.body } }
          : { data: { total: 101 } }
      },
    },
    'case',
    'system',
    101,
    async () => {},
  )
  assert.equal(calls.filter((value) => value === 'post_auth_refresh').length, 1)
  assert.equal(calls[100], 'post_auth_refresh')
})

test('dataset-prepare 私有协议只接受唯一模式和绝对路径', () => {
  const root = path.resolve('.local-tests', 'dataset 私有协议')
  const base = {
    backend_dir: path.resolve('.'),
    format_version: 1,
    kind: 'ryframe-xtask-recovery-dataset-prepare',
    plan: path.join(root, '计划.json'),
    preflight: path.join(root, '预检.json'),
    side: 'source',
    verify_existing: null,
    write: true,
  }
  assert.deepEqual(privateDatasetArguments([], protocolEnvironment(base)), [
    '--plan',
    base.plan,
    '--backend-dir',
    base.backend_dir,
    '--preflight',
    base.preflight,
    '--write',
  ])
  const existing = {
    ...base,
    preflight: null,
    side: 'target',
    verify_existing: path.join(root, '数据.json'),
  }
  assert.deepEqual(privateDatasetArguments([], protocolEnvironment(existing)), [
    '--plan',
    existing.plan,
    '--backend-dir',
    existing.backend_dir,
    '--verify-existing',
    existing.verify_existing,
    '--side',
    'target',
    '--write',
  ])
  for (const candidate of [
    { ...base, unknown: true },
    { ...base, write: false },
    { ...base, side: 'target' },
    { ...base, verify_existing: existing.verify_existing },
    { ...base, preflight: null },
    { ...existing, side: 'unknown' },
    { ...base, plan: 'relative.json' },
  ]) assert.throws(() => privateDatasetArguments([], protocolEnvironment(candidate)))
  assert.throws(() => privateDatasetArguments(['--help'], protocolEnvironment(base)), /不接受/)
  assert.throws(
    () => privateDatasetArguments([], protocolEnvironment(base, {
      RYFRAME_XTASK_RECOVERY_DATASET_PREPARE_EXTRA: 'unexpected',
    })),
    /未知字段/,
  )
})

test('dataset-prepare 私有协议在 JSON 解析前拒绝重复字段', () => {
  const root = path.resolve('.local-tests', 'dataset-protocol').replaceAll('\\', '\\\\')
  const suffix = `"backend_dir":"${path.resolve('.').replaceAll('\\', '\\\\')}",` +
    `"kind":"ryframe-xtask-recovery-dataset-prepare","plan":"${root}\\\\plan.json",` +
    `"preflight":"${root}\\\\preflight.json","side":"source",` +
    '"verify_existing":null,"write":true}'
  for (const repeated of [
    `{"format_version":1,"format_version":1,${suffix}`,
    `{"format_version":1,"format\\u005fversion":1,${suffix}`,
  ]) assert.throws(() => privateDatasetArguments([], protocolEnvironment(repeated)), /字段重复/)
})
