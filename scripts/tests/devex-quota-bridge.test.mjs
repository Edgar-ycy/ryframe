import assert from 'node:assert/strict'
import test from 'node:test'
import path from 'node:path'
import { PassThrough, Readable } from 'node:stream'
import { QuotaSession, quotaAuthorization, quotaProducerArguments, runQuotaBridge, validateQuotaGrant } from '../devex_clone_quota_bridge.mjs'

const backend = process.cwd(), directory = path.join(backend, '.local-tests/quota-test')
const identity = { subject_id: '1', tenant_id: 'system', username: 'admin', password_env: 'RYFRAME_TEST_PASSWORD', client_address: '198.19.20.1' }
const environment = { backend_dir: backend, frontend_dir: path.resolve('../ryframe-vue3'), scope_id: 'quota-fixture',
  api_url: 'http://127.0.0.1:12345/', frontend_url: 'http://127.0.0.1:12346/',
  tenants: Array.from({ length: 10 }, (_, index) => ({ tenant_id: `tenant-${index + 1}` })),
  system_admin: Object.fromEntries(Object.entries(identity).filter(([key]) => key !== 'subject_id')),
  quota: { system_max_users: 200, tenant_max_users: 100 }, pacing: { contract: { authority_sha256: 'a'.repeat(64) }, bindings: {} } }
const argv = ['--run-dir', directory, '--attempt', '1', '--quota-mode', 'apply', '--identity-plan', path.join(directory, 'identity.json'), '--identity-plan-sha256', 'b'.repeat(64)]
const args = quotaProducerArguments(argv)
const grant = () => ({ id: 1, operation: 'initialize', backend, frontend: environment.frontend_dir,
  config: { contract: { timeout_ms: 120000, pacing: environment.pacing.contract }, bindings: {
    scope_id: environment.scope_id, api_url: environment.api_url, frontend_url: environment.frontend_url, pacing: environment.pacing.bindings } },
  identity, artifacts: path.join(directory, 'seed-runtime/attempt-0001/pacing'), run_dir: directory, attempt: 1,
  tenant_ids: ['system', ...environment.tenants.map((item) => item.tenant_id)] })
const auth = () => ({ code: 200, data: { user: { id: '1', tenant_id: 'system', username: 'admin' },
  is_super_admin: true, permissions: [] } })
const row = () => ({ tenant_id: 'system', name: '系统', domain: null, expire_at: null, status: 'enabled',
  max_users: 100, max_roles: 20, max_storage_mb: 1024, max_requests_per_min: 600, usage: { users: { used: 2 } } })
const get = { id: 2, operation: 'get', tenant_id: 'system' }
const update = () => ({ id: 3, operation: 'update', tenant_id: 'system', body: {
  name: '系统', domain: null, expire_at: null, max_users: 200, max_roles: 20, max_storage_mb: 1024, max_requests_per_min: 600 } })
function fixture(options = {}) {
  const events = []
  const session = { token: 'initial', async request(step) {
    events.push(structuredClone(step))
    if (step.operation === options.fail) throw new Error('private secret must not leak')
    if (step.operation === 'get_platform_tenants_by_tenant_id') return { code: 200, data: options.row ?? row() }
    if (step.operation === 'get_platform_tenants_by_tenant_id_usage') return { code: 200, data: { tenant_id: 'system', users: { used: 2 } } }
    if (step.operation === 'put_platform_tenants_by_tenant_id') return { code: 200, data: { ...row(), ...step.body } }
    if (step.operation === 'post_auth_refresh') { if (!options.staleToken) this.token = 'refreshed'; return { code: 200, data: { access_token: this.token } } }
    if (step.operation === 'get_auth_context') return options.auth ?? auth()
    throw new Error('unexpected operation')
  } }
  return { events, session, controller: new QuotaSession(options.mode ?? 'apply', environment, identity, session) }
}

test('参数必须固定 run、attempt、模式及身份计划摘要', () => {
  assert.equal(args.mode, 'apply')
  const change = (key, value) => argv.map((item, index) => index === argv.indexOf(key) + 1 ? value : item)
  for (const invalid of [argv.slice(0, -1), [...argv, '--attempt', '2'], [...argv, '--url', 'http://example.test'],
    ...['0', '1.5', '9007199254740992'].map((value) => change('--attempt', value)),
    change('--run-dir', 'relative'), change('--identity-plan', 'relative'), change('--quota-mode', 'write'), change('--identity-plan-sha256', 'x')])
    assert.throws(() => quotaProducerArguments(invalid))
})

test('初始化逐项核验端点、身份、节奏、十一租户及运行目录', () => {
  validateQuotaGrant(args, grant(), environment)
  for (const mutate of [value => value.tenant_ids.pop(), value => value.tenant_ids.push('unknown'), value => value.tenant_ids[1] = 'system',
    value => value.attempt++, value => value.run_dir += 'other', value => value.artifacts += 'other', value => value.backend += 'other',
    value => value.identity.subject_id = '0', value => value.identity.username = 'other', value => value.identity.tenant_id = 'tenant-1',
    value => value.config.bindings.api_url = 'http://127.0.0.1:9000/', value => value.config.contract.pacing = {},
    value => value.extra = true]) {
    const value = structuredClone(grant()); mutate(value); assert.throws(() => validateQuotaGrant(args, value, environment))
  }
  const value = structuredClone(grant()); value.config.bindings.api_url = value.config.bindings.api_url.slice(0, -1)
  validateQuotaGrant(args, value, environment)
})

test('完整原始读取响应保留，唯一 PUT 后显式刷新再核验权威上下文', async () => {
  const { controller, events, session } = fixture()
  assert.deepEqual(await controller.operation(get), { code: 200, data: row() })
  assert.equal((await controller.operation({ ...get, operation: 'usage' })).data.users.used, 2)
  const result = await controller.operation(update())
  assert.equal(result.response.data.max_users, 200); assert.equal(result.authorization.subject_id, '1'); assert.equal(session.token, 'refreshed')
  assert.deepEqual(events.map(item => item.operation), ['get_platform_tenants_by_tenant_id', 'get_platform_tenants_by_tenant_id_usage',
    'put_platform_tenants_by_tenant_id', 'post_auth_refresh', 'get_auth_context'])
  await assert.rejects(controller.operation(update()))
  assert.equal(events.filter(item => item.operation.startsWith('put_')).length, 1)
})

for (const mode of ['plan', 'reconcile']) test(`${mode} 模式不得写入`, async () => {
  const { controller, events } = fixture({ mode }); await controller.operation(get)
  await assert.rejects(controller.operation(update())); assert.equal(events.length, 1)
})

test('只允许登记租户、显式配额和完整六字段原值，未知动作关闭会话', async () => {
  for (const mutate of [value => value.body.max_users = 201, value => value.body.max_users = 100,
    value => value.body.max_roles++, value => value.body.name = '其他', value => value.body.domain = 'other',
    value => value.body.expire_at = '2099', value => value.body.max_storage_mb++, value => value.body.max_requests_per_min++,
    value => value.body.alias = true, value => delete value.body.domain, value => value.tenant_id = 'unknown', value => value.operation = 'request']) {
    const { controller, events } = fixture(); await controller.operation(get)
    const value = update(); mutate(value); await assert.rejects(controller.operation(value)); assert.equal(events.length, 1)
  }
  const { controller, events } = fixture(); await assert.rejects(controller.operation(update())); assert.equal(events.length, 0)
})

for (const fail of ['put_platform_tenants_by_tenant_id', 'post_auth_refresh', 'get_auth_context'])
  test(`${fail} 失败不能重放 PUT 或继续读取`, async () => {
    const { controller, events } = fixture({ fail }); await controller.operation(get)
    await assert.rejects(controller.operation(update())); const count = events.length
    await assert.rejects(controller.operation(update())); await assert.rejects(controller.operation(get))
    assert.equal(events.length, count); assert.equal(events.filter(item => item.operation.startsWith('put_')).length, 1)
  })

test('刷新未替换 token 或权限降低均留下未知更新', async () => {
  for (const options of [{ staleToken: true }, { auth: { data: { ...auth().data, is_super_admin: false } } }]) {
    const { controller, events } = fixture(options); await controller.operation(get)
    await assert.rejects(controller.operation(update())); await assert.rejects(controller.operation(update()))
    assert.equal(events.filter(item => item.operation.startsWith('put_')).length, 1)
  }
})

test('权威管理员或明确三权限可用，其他身份和缺少权限关闭', () => {
  assert.equal(quotaAuthorization(auth(), identity).is_super_admin, true)
  for (const permissions of [['*'], ['tenant:list', 'tenant:edit', 'tenant:usage:list']])
    assert.deepEqual(quotaAuthorization({ data: { ...auth().data, is_super_admin: false, permissions } }, identity).permissions, permissions)
  for (const mutate of [value => value.data.user.id = '2', value => value.data.user.tenant_id = 'other',
    value => value.data.user.username = 'other', value => value.data.permissions = null,
    value => { value.data.is_super_admin = false; value.data.permissions = ['tenant:list', 'tenant:edit'] }]) {
    const value = auth(); mutate(value); assert.throws(() => quotaAuthorization(value, identity))
  }
})

test('没有 grant、错误 run 或 attempt 时不得加载 API 或计划', async () => {
  let loads = 0, plans = 0
  const dependencies = async () => { loads++; throw new Error('must not load') }
  const readPlan = async () => { plans++; return environment }
  for (const messages of [[], [{ ...grant(), run_dir: directory + '-other' }], [{ ...grant(), attempt: 2 }]]) {
    const output = new PassThrough(); await runQuotaBridge(argv, Readable.from(messages.map(value => JSON.stringify(value) + '\n')), output, dependencies, readPlan)
  }
  assert.equal(loads, 0); assert.equal(plans, 0)
  const input = new PassThrough(), output = new PassThrough(), pending = runQuotaBridge(argv, input, output, dependencies, readPlan)
  await new Promise(resolve => setImmediate(resolve)); assert.equal(loads, 0); assert.equal(plans, 0)
  input.end(); await pending
})

test('正确 grant 才登录；请求序号或刷新失败终止并隐藏秘密', async () => {
  let loads = 0, logouts = 0, pacingClosed = 0, logins = 0
  const { session, events } = fixture({ fail: 'post_auth_refresh' })
  const load = async () => { loads++; return { Session: class { constructor() { return session } }, operationCatalog: async () => new Map(),
    createPacing: async () => ({ createPreparationControls: async () => ({}), close: async () => { pacingClosed++ } }) } }
  session.login = async () => { logins++ }
  const request = session.request.bind(session)
  session.request = async step => { if (step.operation === 'post_auth_logout') { logouts++; return {} } return request(step) }
  const output = new PassThrough(); let text = ''; output.on('data', value => { text += value })
  await runQuotaBridge(argv, Readable.from([grant(), get, update(), { ...update(), id: 4 }].map(value => JSON.stringify(value) + '\n')),
    output, load, async () => environment)
  const responses = text.trim().split('\n').map(JSON.parse)
  assert.equal(responses.length, 3); assert.equal(responses[2].ok, false); assert.equal(text.includes('private secret'), false)
  assert.equal(events.filter(item => item.operation.startsWith('put_')).length, 1)
  assert.equal(loads, 1); assert.equal(logins, 1); assert.equal(logouts, 1); assert.equal(pacingClosed, 1)
})


test('完整配额前像中的不安全整数或非整数不得经过 PUT 回写', async () => {
  for (const key of ['max_users', 'max_roles', 'max_storage_mb', 'max_requests_per_min']) {
    for (const value of [Number.MAX_SAFE_INTEGER + 1, -1, true, 1.5]) {
      const fixtureRow = { ...row(), [key]: value }
      const { controller, events } = fixture({ row: fixtureRow })
      await assert.rejects(controller.operation(get)); await assert.rejects(controller.operation(update()))
      assert.equal(events.filter(item => item.operation.startsWith('put_')).length, 0)
    }
  }
})
