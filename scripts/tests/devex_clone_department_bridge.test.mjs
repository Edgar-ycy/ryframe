import assert from 'node:assert/strict'
import test from 'node:test'
import path from 'node:path'
import { PassThrough, Readable } from 'node:stream'
import { createHash } from 'node:crypto'
import { DepartmentSession, classifyDepartmentList, departmentAuthorization, departmentBody,
  departmentName, departmentTargets, departmentTemplateAuthorization, departmentTemplateIdentities,
  departmentTemplateResponse, validateDepartmentDetail, validateDepartmentGrant } from '../devex/department-stage.mjs'
import { departmentProducerArguments, runDepartmentBridge } from '../devex_clone_department_bridge.mjs'
import { Session } from '../devex/request.mjs'

const backend = process.cwd()
const directory = path.join(backend, '.local-tests/department-test')
const admin = (tenant_id, index) => ({ tenant_id, username: 'admin', password_env: 'RYFRAME_DEPARTMENT_PASSWORD',
  client_address: `198.18.31.${index}` })
const systemAdmin = admin('system', 1)
const tenants = Array.from({ length: 10 }, (_, index) => ({ slot: `tenant-${String(index + 1).padStart(2, '0')}`,
  tenant_id: `fixture-${index + 1}`, target_key: 'shared', admin: admin(`fixture-${index + 1}`, index + 2) }))
const plan = { environment: { plan_id: 'fixture', scope_id: 'department-fixture', backend_dir: backend,
  frontend_dir: path.resolve('../ryframe-vue3'), api_url: 'http://127.0.0.1:12345/', frontend_url: 'http://127.0.0.1:12346/',
  pacing: { contract: { authority_sha256: 'a'.repeat(64) }, bindings: {} }, system_admin: systemAdmin, tenants },
groups: [{ slot: 'system', tenant_id: 'system', kind: 'system', admin: systemAdmin },
  ...tenants.map((item, index) => ({ slot: item.slot, tenant_id: item.tenant_id, kind: 'tenant', admin: item.admin,
    permissions: ['system:post:list', 'system:user-import:add'], users: [{ user_slot: `${item.slot}-user-001`,
      username: `fixed-user-${index + 1}`, password_env: 'RYFRAME_FIXED_USER_PASSWORD',
      client_address: `198.18.41.${index + 1}` }] }))] }
const templateIdentities = plan.groups.slice(1).map((group, index) => ({ subject_id: String(101 + index),
  tenant_id: group.tenant_id, username: group.users[0].username, password_env: group.users[0].password_env,
  client_address: group.users[0].client_address }))
const argv = ['--run-dir', directory, '--attempt', '7', '--department-mode', 'apply',
  '--identity-plan', path.join(directory, 'identity.json'), '--identity-plan-sha256', 'b'.repeat(64)]
const args = departmentProducerArguments(argv)
const identity = { subject_id: '1', ...systemAdmin }
const grant = () => ({ id: 1, operation: 'initialize', backend, frontend: plan.environment.frontend_dir,
  config: { contract: { timeout_ms: 120000, pacing: plan.environment.pacing.contract }, bindings: {
    scope_id: plan.environment.scope_id, api_url: plan.environment.api_url,
    frontend_url: plan.environment.frontend_url, pacing: plan.environment.pacing.bindings } }, identity,
  artifacts: path.join(directory, 'seed-runtime/attempt-0007/pacing'), run_dir: directory, attempt: 7,
  tenant_ids: tenants.map((item) => item.tenant_id), template_identities: templateIdentities })
const authorization = (target, permissions = []) => ({ code: 200, data: { user: {
  id: target.subject_id ?? '9', tenant_id: target.tenant_id,
  username: target.username }, is_super_admin: permissions.length === 0, permissions } })
const record = (id = '101') => ({ id, name: departmentName(plan), parent_id: null, ancestors: '0', sort: 0,
  status: '1', remark: null, created_at: '2026-09-05T00:00:00Z' })
const idempotencyKey = '12345678-1234-4123-8123-123456789abc'
const page = (items = []) => ({ code: 200, data: { items, page: 1, page_size: 100, total: items.length,
  total_pages: items.length ? 1 : 0, max_page_size: 100 } })
const template = (content = Buffer.from('fixture-xlsx')) => ({ bytes: content.length,
  media_type: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  sha256: createHash('sha256').update(content).digest('hex'), base64: content.toString('base64') })

function controllerFixture(mode = 'apply', options = {}) {
  const events = [], ownerEvents = [], templateEvents = []
  let rows = options.rows ?? []
  const request = async (purpose, step) => {
    events.push(structuredClone(step))
    ;(purpose === 'owner' ? ownerEvents : templateEvents).push(structuredClone(step))
    if (step.operation === options.fail) throw new Error('private-secret-must-not-leak')
    if (step.operation === 'get_system_depts') return page(rows)
    if (step.operation === 'get_system_depts_by_id') return { code: 200, data: rows.find((item) => item.id === step.path.id) }
    if (step.operation === 'get_system_users_import_template') return template()
    if (step.operation === 'post_system_depts') { rows = [record()]; return { code: 200, data: rows[0] } }
    throw new Error('unexpected operation')
  }
  const sessions = { owner: { request: (step) => request('owner', step) },
    template: { request: (step) => request('template', step) } }
  return { events, ownerEvents, templateEvents, rows: () => rows,
    controller: new DepartmentSession(mode, plan, templateIdentities, async (target, purpose) => ({
      session: sessions[purpose], authorization: purpose === 'owner'
        ? { subject_id: '9', tenant_id: target.tenant_id, username: 'admin', is_super_admin: true, permissions: [] }
        : { subject_id: target.template_identity.subject_id, tenant_id: target.tenant_id,
          username: target.template_identity.username, is_super_admin: false,
          permissions: ['system:user-import:add'] },
    })) }
}

test('生产者参数固定run、attempt、四种模式和身份计划摘要', () => {
  assert.equal(args.mode, 'apply')
  const change = (key, value) => argv.map((item, index) => index === argv.indexOf(key) + 1 ? value : item)
  for (const invalid of [argv.slice(0, -1), [...argv, '--attempt', '8'], [...argv, '--url', 'http://example.test'],
    ...['0', '1.5', '9007199254740992'].map((value) => change('--attempt', value)), change('--run-dir', 'relative'),
    change('--identity-plan', 'relative'), change('--department-mode', 'write'), change('--identity-plan-sha256', 'x')])
    assert.throws(() => departmentProducerArguments(invalid))
  for (const mode of ['plan', 'apply', 'reconcile', 'verify'])
    assert.equal(departmentProducerArguments(change('--department-mode', mode)).mode, mode)
})

test('十租户、固定同名根部门和初始化grant均由身份计划决定', () => {
  assert.equal(departmentTargets(plan).length, 10)
  assert.equal(departmentTemplateIdentities(plan, templateIdentities)[0].template_identity.subject_id, '101')
  assert.equal(departmentName(plan), 'dv-fixture-import')
  assert.deepEqual(departmentBody(plan), { name: 'dv-fixture-import', parent_id: null, sort: 0 })
  validateDepartmentGrant(args, grant(), plan)
  for (const mutate of [value => value.tenant_ids.pop(), value => value.tenant_ids.push('unknown'),
    value => value.tenant_ids.reverse(), value => value.attempt++, value => value.run_dir += '-other',
    value => value.artifacts += '-other', value => value.backend += '-other', value => value.identity.username = 'other',
    value => value.template_identities[0].subject_id = '0',
    value => value.template_identities[0].username = 'other', value => value.template_identities.pop(),
    value => value.config.bindings.api_url = 'http://127.0.0.1:9000/', value => value.extra = true]) {
    const value = structuredClone(grant()); mutate(value); assert.throws(() => validateDepartmentGrant(args, value, plan))
  }
})

test('apply只允许完整空前像后每租户一次固定POST，并可只读核对详情', async () => {
  const { controller, events, rows } = controllerFixture()
  const tenant_id = tenants[0].tenant_id, name = departmentName(plan)
  assert.equal(classifyDepartmentList((await controller.operation({ id: 2, operation: 'list', tenant_id, name })).response, name).state, 'before')
  const created = await controller.operation({ id: 3, operation: 'create', tenant_id, body: departmentBody(plan),
    idempotency_key: idempotencyKey })
  validateDepartmentDetail(created.response, name, '101')
  await controller.operation({ id: 4, operation: 'detail', tenant_id, department_id: '101' })
  assert.equal(rows().length, 1)
  await assert.rejects(controller.operation({ id: 5, operation: 'create', tenant_id, body: departmentBody(plan),
    idempotency_key: idempotencyKey }))
  assert.equal(events.filter((item) => item.operation === 'post_system_depts').length, 1)
  assert.equal(events.find((item) => item.operation === 'post_system_depts').idempotency_key, idempotencyKey)
})

test('导入模板只通过固定只读operation捕获且校验长度、媒体类型和摘要', async () => {
  const { controller, events, ownerEvents, templateEvents } = controllerFixture('plan')
  const result = await controller.operation({ id: 2, operation: 'template', tenant_id: tenants[0].tenant_id })
  assert.deepEqual(departmentTemplateResponse(result.response), template())
  assert.equal(result.authorization.subject_id, templateIdentities[0].subject_id)
  assert.deepEqual(events.at(-1), { operation: 'get_system_users_import_template', binary: true })
  assert.equal(ownerEvents.length, 0)
  assert.equal(templateEvents.length, 1)
  for (const mutate of [value => value.bytes++, value => value.sha256 = '0'.repeat(64),
    value => value.media_type = 'application/json', value => value.base64 += '=']) {
    const value = template(); mutate(value); assert.throws(() => departmentTemplateResponse(value))
  }
})

test('固定部门写入键精确进入HTTP头，其他operation或非UUIDv4均拒绝', async () => {
  const config = { bindings: { api_url: 'http://127.0.0.1:3000', frontend_url: 'http://127.0.0.1:4190' },
    contract: { timeout_ms: 1000 } }
  const catalog = new Map([['post_system_depts', { method: 'POST', path: '/system/depts' }],
    ['put_system_depts', { method: 'PUT', path: '/system/depts' }]])
  const session = new Session(config, catalog, { tenant_id: tenants[0].tenant_id })
  let received
  const original = globalThis.fetch
  globalThis.fetch = async (_url, request) => {
    received = request.headers
    return new Response(JSON.stringify({ code: 200, data: record() }), {
      headers: { 'content-type': 'application/json' },
    })
  }
  try {
    await session.request({ operation: 'post_system_depts', body: departmentBody(plan), idempotency_key: idempotencyKey })
    assert.equal(received['Idempotency-Key'], idempotencyKey)
    await assert.rejects(session.request({ operation: 'post_system_depts', body: {}, idempotency_key: 'invalid' }))
    await assert.rejects(session.request({ operation: 'put_system_depts', body: {}, idempotency_key: idempotencyKey }))
  } finally { globalThis.fetch = original }
})

for (const mode of ['plan', 'reconcile', 'verify']) test(`${mode}模式禁止部门写入`, async () => {
  const { controller, events } = controllerFixture(mode)
  const tenant_id = tenants[0].tenant_id, name = departmentName(plan)
  await controller.operation({ id: 2, operation: 'list', tenant_id, name })
  await assert.rejects(controller.operation({ id: 3, operation: 'create', tenant_id, body: departmentBody(plan),
    idempotency_key: idempotencyKey }))
  assert.equal(events.filter((item) => item.operation === 'post_system_depts').length, 0)
})

test('非空目录、错后像及失败请求均不开放或重放POST', async () => {
  for (const rows of [[record()], [{ ...record(), status: '0' }], [record(), record('102')]]) {
    const { controller, events } = controllerFixture('apply', { rows })
    const tenant_id = tenants[0].tenant_id, name = departmentName(plan)
    const observed = await controller.operation({ id: 2, operation: 'list', tenant_id, name })
    assert.notEqual(classifyDepartmentList(observed.response, name).state, 'before')
    await assert.rejects(controller.operation({ id: 3, operation: 'create', tenant_id, body: departmentBody(plan),
      idempotency_key: idempotencyKey }))
    assert.equal(events.filter((item) => item.operation === 'post_system_depts').length, 0)
  }
  const { controller, events } = controllerFixture('apply', { fail: 'post_system_depts' })
  const tenant_id = tenants[0].tenant_id, name = departmentName(plan)
  await controller.operation({ id: 2, operation: 'list', tenant_id, name })
  await assert.rejects(controller.operation({ id: 3, operation: 'create', tenant_id, body: departmentBody(plan),
    idempotency_key: idempotencyKey }))
  const count = events.length
  await assert.rejects(controller.operation({ id: 4, operation: 'create', tenant_id, body: departmentBody(plan),
    idempotency_key: idempotencyKey }))
  assert.equal(events.length, count)
  assert.equal(events.filter((item) => item.operation === 'post_system_depts').length, 1)
})

test('租户管理员必须匹配且具备模式所需权限', () => {
  const target = tenants[0].admin
  assert.equal(departmentAuthorization(authorization(target), target, 'apply').is_super_admin, true)
  assert.deepEqual(departmentAuthorization(authorization(target, ['system:dept:list', 'system:dept:add']), target, 'apply').permissions,
    ['system:dept:list', 'system:dept:add'])
  assert.deepEqual(departmentAuthorization(authorization(target, ['system:dept:list']), target, 'verify').permissions, ['system:dept:list'])
  const wildcard = authorization(target, ['*']); wildcard.data.is_super_admin = false
  assert.throws(() => departmentAuthorization(wildcard, target, 'apply'))
  const mixed = authorization(target, ['*', 'system:dept:list', 'system:dept:add']); mixed.data.is_super_admin = false
  assert.throws(() => departmentAuthorization(mixed, target, 'apply'))
  const superWildcard = authorization(target, ['*'])
  assert.throws(() => departmentAuthorization(superWildcard, target, 'apply'))
  for (const mutate of [value => value.data.user.id = '0', value => value.data.user.tenant_id = 'other',
    value => value.data.user.username = 'other', value => value.data.permissions = null,
    value => { value.data.is_super_admin = false; value.data.permissions = ['system:dept:list'] }]) {
    const value = authorization(target); mutate(value); assert.throws(() => departmentAuthorization(value, target, 'apply'))
  }
})

test('模板用户必须匹配账本主体且明确具备导入权限', () => {
  const identity = templateIdentities[0]
  const valid = authorization(identity, ['system:user-import:add'])
  assert.equal(departmentTemplateAuthorization(valid, identity).subject_id, identity.subject_id)
  for (const mutate of [value => value.data.user.id = '999', value => value.data.user.tenant_id = 'other',
    value => value.data.user.username = 'other', value => value.data.permissions = [],
    value => value.data.permissions = ['*', 'system:user-import:add'], value => value.data.is_super_admin = true]) {
    const current = structuredClone(valid); mutate(current)
    assert.throws(() => departmentTemplateAuthorization(current, identity))
  }
})

test('正确grant后才加载会话；协议只输出结构化错误且清理会话', async () => {
  let loads = 0, plans = 0, logins = 0, logouts = 0, pacingClosed = 0
  class Session {
    constructor(_config, _catalog, identityValue) { this.identity = identityValue; this.token = undefined }
    async login() { logins++; this.token = 'private-secret-token' }
    async request(step) {
      if (step.operation === 'get_auth_context') return this.identity.subject_id
        ? authorization(this.identity, ['system:user-import:add']) : authorization(this.identity)
      if (step.operation === 'get_system_depts') return page()
      if (step.operation === 'get_system_users_import_template') return template()
      if (step.operation === 'post_auth_logout') { logouts++; this.token = undefined; return { code: 200, data: null } }
      throw new Error('private-secret-response')
    }
  }
  const load = async () => { loads++; return { Session, operationCatalog: async () => new Map(),
    createPacing: async () => ({ createPreparationControls: async () => ({}), close: async () => { pacingClosed++ } }) } }
  const readPlan = async () => { plans++; return plan }
  for (const message of [{ ...grant(), run_dir: directory + '-other' }, { ...grant(), attempt: 8 }]) {
    const output = new PassThrough()
    await runDepartmentBridge(argv, Readable.from([JSON.stringify(message) + '\n']), output, load, readPlan)
  }
  assert.equal(loads, 0); assert.equal(plans, 0)
  const list = { id: 2, operation: 'list', tenant_id: tenants[0].tenant_id, name: departmentName(plan) }
  const templateRequest = { id: 3, operation: 'template', tenant_id: tenants[0].tenant_id }
  const output = new PassThrough(); let text = ''; output.on('data', (chunk) => { text += chunk })
  await runDepartmentBridge(argv, Readable.from([grant(), list, templateRequest, { id: 4, operation: 'close' }]
    .map((item) => JSON.stringify(item) + '\n')), output, load, readPlan)
  const responses = text.trim().split('\n').map(JSON.parse)
  assert.equal(responses.length, 4); assert.ok(responses.every((item) => item.ok))
  assert.equal(loads, 1); assert.equal(plans, 1); assert.equal(logins, 2); assert.equal(logouts, 2); assert.equal(pacingClosed, 1)
  assert.equal(text.includes('private-secret'), false)
})

test('初始化失败的悬挂补偿注销有界且迟到完成不产生第二响应', async () => {
  let logouts = 0, pacingClosed = 0, finishLogout
  class Session {
    constructor() { this.token = undefined }
    async login() { this.token = 'private-token' }
    async request(step) {
      if (step.operation === 'get_auth_context') throw new Error('private-auth-failure')
      if (step.operation === 'post_auth_logout') {
        logouts++
        return new Promise((resolve) => { finishLogout = resolve })
      }
      throw new Error('unexpected operation')
    }
  }
  const load = async () => ({ Session, operationCatalog: async () => new Map(),
    createPacing: async () => ({ createPreparationControls: async () => ({}),
      close: async () => { pacingClosed++ } }) })
  const list = { id: 2, operation: 'list', tenant_id: tenants[0].tenant_id, name: departmentName(plan) }
  const output = new PassThrough(); let text = ''; output.on('data', (chunk) => { text += chunk })
  const started = Date.now()
  await runDepartmentBridge(argv, Readable.from([grant(), list]
    .map((item) => JSON.stringify(item) + '\n')), output, load, async () => plan, 5)
  const responses = text.trim().split('\n').map(JSON.parse)
  assert.equal(responses.length, 2); assert.equal(responses[0].ok, true)
  assert.equal(responses[1].ok, false); assert.equal(responses[1].error_type, 'AggregateError')
  assert.equal(logouts, 1); assert.equal(pacingClosed, 1); assert.ok(Date.now() - started < 1000)
  const completed = text
  finishLogout({ code: 200, data: null })
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(text, completed); assert.equal(text.includes('private'), false)
})

test('关闭为每个已绑定会话和pacing保留独立有限预算', async () => {
  let logouts = 0, pacingClosed = 0
  class Session {
    constructor(_config, _catalog, identityValue) { this.identity = identityValue; this.token = undefined }
    async login() { this.token = 'private-token' }
    async request(step) {
      if (step.operation === 'get_auth_context') return this.identity.subject_id
        ? authorization(this.identity, ['system:user-import:add']) : authorization(this.identity)
      if (step.operation === 'get_system_depts') return page()
      if (step.operation === 'get_system_users_import_template') return template()
      if (step.operation === 'post_auth_logout') { logouts++; return new Promise(() => {}) }
      throw new Error('unexpected operation')
    }
  }
  const load = async () => ({ Session, operationCatalog: async () => new Map(),
    createPacing: async () => ({ createPreparationControls: async () => ({}),
      close: async () => { pacingClosed++; return new Promise(() => {}) } }) })
  const list = { id: 2, operation: 'list', tenant_id: tenants[0].tenant_id, name: departmentName(plan) }
  const templateRequest = { id: 3, operation: 'template', tenant_id: tenants[0].tenant_id }
  const output = new PassThrough(); let text = ''; output.on('data', (chunk) => { text += chunk })
  const started = Date.now()
  await runDepartmentBridge(argv, Readable.from([grant(), list, templateRequest, { id: 4, operation: 'close' }]
    .map((item) => JSON.stringify(item) + '\n')), output, load, async () => plan, 5)
  const responses = text.trim().split('\n').map(JSON.parse)
  assert.equal(responses.length, 4); assert.ok(responses.slice(0, 3).every((item) => item.ok))
  assert.equal(responses[3].ok, false); assert.equal(responses[3].error_type, 'AggregateError')
  assert.equal(logouts, 2); assert.equal(pacingClosed, 1); assert.ok(Date.now() - started < 1000)
})
