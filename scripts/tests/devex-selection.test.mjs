import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises'
import path from 'node:path'
import { load } from '../devex/load.mjs'
import { RequestFailure } from '../devex/failure.mjs'
import { hash } from '../devex/config.mjs'
import { selectWorkloadIdentities, validateSuiteSelection } from '../devex/selection.mjs'
import { identityPool, selectionConfig } from './devex-selection-fixture.mjs'

test('实际前 N 身份独立，尾部未选中的用户不能补齐或改变结果', () => {
  const pool = identityPool(3, { selections: [2] })
  const config = selectionConfig(pool)
  const result = selectWorkloadIdentities(config, 'api', 'list', 2)
  assert.equal(result.length, 2)
  pool.binding[2] = null
  assert.deepEqual(selectWorkloadIdentities(config, 'api', 'list', 2), result)
  pool.binding[1] = pool.binding[0]
  assert.throws(() => selectWorkloadIdentities(config, 'api', 'list', 2), /独立用户/)
  pool.binding.pop(); pool.binding.pop()
  assert.throws(() => selectWorkloadIdentities(config, 'api', 'list', 2), /缺少/)
  assert.throws(() => selectWorkloadIdentities(config, 'api', 'list', 0), /数量无效/)
})

test('10、50、100并发的实际前N用户均匀覆盖十租户，导入仍真实并发', () => {
  const pool = identityPool(100, { tenants: 10, selections: [10, 50, 100] })
  for (const [suite, workflow] of [['tenants', 'export'], ['jobs', 'import']]) {
    const config = selectionConfig(pool, suite, workflow)
    for (const concurrency of [10, 50, 100]) {
      const selected = selectWorkloadIdentities(config, suite, workflow, concurrency)
      assert.equal(selected.length, concurrency)
      assert.equal(new Set(selected.map((value) => value.tenant_id)).size, 10)
      const receipt = validateSuiteSelection(config, suite, concurrency)
      assert.ok(Object.values(receipt.selections[0].tenant_distribution).every((value) => value === concurrency / 10))
    }
    const wrong = structuredClone(config)
    wrong.bindings.identity_pools.readers[9].tenant_id = 'actual-tenant-1'
    assert.throws(() => validateSuiteSelection(wrong, suite, 10), /一一对应/)
    const nine = selectionConfig(identityPool(100, { tenants: 9, selections: [10] }), suite, workflow)
    assert.throws(() => validateSuiteSelection(nine, suite, 10), /十个普通租户/)
  }
})

test('每个job场景选择自身池，Schedule只允许system，所有场景预检先于副作用', async () => {
  const imports = identityPool(10, { tenants: 10 })
  const system = identityPool(10, { system: true })
  const config = { contract: { cycles: 1, identity_pools: { imports: imports.contract, system: system.contract },
    workloads: { jobs: [{ name: 'import', identity_pool: 'imports', job: { kind: 'import' } },
      { name: 'schedule', identity_pool: 'system', job: { kind: 'schedule' } }] } },
  bindings: { identity_pools: { imports: imports.binding, system: system.binding } } }
  assert.ok(selectWorkloadIdentities(config, 'jobs', 'import', 10).every((value) => value.tenant_id !== 'system'))
  assert.ok(selectWorkloadIdentities(config, 'jobs', 'schedule', 10).every((value) => value.tenant_id === 'system'))
  config.contract.workloads.jobs[1].identity_pool = 'imports'
  let starts = 0
  await assert.rejects(load(config, {}, 'jobs', 10, 'unused', {
    Session: class { constructor() { starts++ } }, jobWaiter: async () => { starts++ },
  }), /system/)
  assert.equal(starts, 0)
})

test('旧结构、缺失摘要、重复地址、错误分布或绑定均失败关闭', () => {
  for (const change of [
    (value) => { value.bindings.identities = [] },
    (value) => { delete value.contract.identity_pools.readers.roles_sha256 },
    (value) => { value.contract.identity_pools.readers.permissions_sha256 = 'invalid' },
    (value) => { value.contract.identity_pools.readers.client_address_model = 'rotating' },
    (value) => { value.contract.identity_pools.readers.slots[1].client_address = '198.18.10.1' },
    (value) => { value.contract.identity_pools.readers.slots[0].client_address = '203.0.113.1' },
    (value) => { value.contract.identity_pools.readers.slots[1].user_slot = 'user-1' },
    (value) => { value.contract.identity_pools.readers.selections['10'] = { 'tenant-1': 9 } },
    (value) => { value.bindings.identity_pools.readers[1].tenant_id = 'system' },
    (value) => { value.bindings.identity_pools.readers[1].password_env = 'secret-value' },
    (value) => { value.bindings.identity_pools.readers[1].token = 'forbidden' },
    (value) => { value.bindings.identity_pools.readers[1] = {} },
    (value) => { value.contract.workloads.api[0].identity_pool = 'missing' },
  ]) {
    const config = selectionConfig(identityPool(10))
    change(config)
    assert.throws(() => validateSuiteSelection(config, 'api', 10))
  }
})

test('角色权限摘要、数量、分布及固定地址改变负载hash，选择收据不泄漏实际身份', () => {
  const config = selectionConfig(identityPool(10))
  const original = hash(config.contract)
  for (const change of [
    (value) => { value.identity_pools.readers.roles_sha256 = 'c'.repeat(64) },
    (value) => { value.identity_pools.readers.permissions_sha256 = 'd'.repeat(64) },
    (value) => { value.identity_pools.readers.selections['10']['tenant-1'] = 9 },
    (value) => { value.identity_pools.readers.slots[0].client_address = '198.18.20.1' },
  ]) {
    const contract = structuredClone(config.contract)
    change(contract)
    assert.notEqual(hash(contract), original)
  }
  const receipt = JSON.stringify(validateSuiteSelection(config, 'api', 10))
  assert.ok(!receipt.includes('actual-tenant') && !receipt.includes('password_env') && !receipt.includes('RYFRAME_SELECTION'))
  assert.ok(receipt.includes('roles_sha256') && receipt.includes('client_address'))
})

test('独立池真实保持10/50/100个并行调用，容量409计失败且不会重试或改池', async (t) => {
  const root = path.resolve('.local-tests')
  await mkdir(root, { recursive: true })
  const directory = await mkdtemp(path.join(root, 'selection-admission-'))
  t.after(async () => {
    assert.equal(path.dirname(directory), root)
    await rm(directory, { recursive: true })
  })
  for (const concurrency of [10, 50, 100]) {
    const imports = identityPool(concurrency, { tenants: 10 })
    const system = identityPool(concurrency, { system: true })
    const config = { contract: { cycles: 1, identity_pools: { imports: imports.contract, system: system.contract },
      workloads: { jobs: [{ name: 'import', identity_pool: 'imports' }, { name: 'schedule', identity_pool: 'system' }] } },
    bindings: { identity_pools: { imports: imports.binding, system: system.binding } } }
    const active = new Set(), calls = []
    let firstCompletionCalls
    class Session {
      requests = 0
      constructor(_config, _catalog, identity) { this.identity = identity }
      async login() {}
      async request() {}
      async workflow(workflow) {
        calls.push({ workflow: workflow.name, tenant: this.identity.tenant_id })
        if (workflow.name === 'schedule') return
        if (active.has(this.identity.tenant_id))
          throw new RequestFailure('http_status', 'post_system_user_imports', 409)
        active.add(this.identity.tenant_id)
        await new Promise((resolve) => setImmediate(resolve))
        firstCompletionCalls ??= calls.length
        active.delete(this.identity.tenant_id)
      }
    }
    const artifacts = path.join(directory, String(concurrency))
    const result = await load(config, {}, 'jobs', concurrency, artifacts, { Session })
    await result.finalize()
    assert.equal(firstCompletionCalls, concurrency)
    assert.equal(calls.length, concurrency * 2)
    assert.ok(calls.filter((call) => call.workflow === 'schedule').every((call) => call.tenant === 'system'))
    assert.equal(result.scenarios.import.completed, 10)
    assert.equal(result.scenarios.import.failed, concurrency - 10)
    assert.equal(result.scenarios.schedule.completed, concurrency)
    const rows = (await readFile(path.join(artifacts, 'cycles.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse)
    assert.equal(rows.filter((row) => row.failure?.status === 409).length, concurrency - 10)
  }
})
