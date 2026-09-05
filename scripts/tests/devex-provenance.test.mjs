import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { verifyProvenance } from '../devex/provenance.mjs'
import { measure } from '../devex/measure.mjs'
import { identityPool } from './devex-selection-fixture.mjs'

const config = () => ({ contract: { environment_sha256: 'a'.repeat(64), cycles: 10,
  identity_pools: { users: identityPool(10).contract }, homepage: { identity_pool: 'users' } }, bindings: {
  identity_pools: { users: identityPool(10).binding },
  provenance: { python: process.execPath }, source_fingerprints: { backend: 'b', frontend: 'f' },
} })
const values = () => ({
  backend: process.cwd(),
  frontend: process.cwd(),
  driver: process.cwd(),
  driver_fingerprint: `sha256:${'d'.repeat(64)}`,
  suite: 'homepage',
})
const receipt = () => ({ format_version: 1, kind: 'devex-runtime-provenance',
  processes: { api: { pid: 42, started: 'original' } } })
async function directory(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), 'devex-provenance-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  return root
}

test('前后验证绑定同一请求快照，保存独立来源收据与校验耗时', async (t) => {
  const root = await directory(t), current = config(), requests = []
  const after = await verifyProvenance(current, values(), root, async (_python, request) => {
    requests.push(structuredClone(request)); return receipt()
  })
  current.bindings.source_fingerprints.backend = 'changed'
  await after()
  assert.deepEqual(requests[0], requests[1])
  assert.equal(requests[0].driver, process.cwd())
  assert.equal(requests[0].driver_fingerprint, `sha256:${'d'.repeat(64)}`)
  for (const phase of ['before', 'after']) {
    const result = JSON.parse(await readFile(path.join(root, `provenance-${phase}.json`)))
    assert.equal(result.success, true)
    assert.ok(result.duration_ms >= 0)
    assert.deepEqual(result.receipt, receipt())
  }
})

test('前后进程身份变化或后验异常均保存失败，不能成为成功性能样本', async (t) => {
  for (const mode of ['changed', 'exception']) {
    const root = path.join(await directory(t), mode)
    let calls = 0
    const after = await verifyProvenance(config(), values(), root, async () => {
      if (++calls === 1) return receipt()
      if (mode === 'exception') throw new Error('processes')
      return { ...receipt(), processes: { api: { pid: 42, started: 'restarted' } } }
    })
    await assert.rejects(after(), mode === 'changed' ? /sample_changed/ : /processes/)
    const result = JSON.parse(await readFile(path.join(root, 'provenance-after.json')))
    assert.equal(result.success, false)
  }
})

test('缺少显式来源绑定与失败原始异常都落盘，报告不泄露参数', async (t) => {
  const root = await directory(t)
  await assert.rejects(verifyProvenance({ bindings: {}, contract: {} }, values(), root), /bindings/)
  assert.equal(JSON.parse(await readFile(path.join(root, 'provenance-before.json'))).failure, 'bindings')
  const other = await directory(t)
  await assert.rejects(verifyProvenance(config(), values(), other, async () => {
    throw new Error('password=secret; endpoint=private')
  }), /verification_failed/)
  const text = await readFile(path.join(other, 'provenance-before.json'), 'utf8')
  assert.equal(text.includes('secret'), false)
  assert.equal(text.includes('private'), false)
})

test('driver 路径与完整指纹必须显式绑定', async (t) => {
  for (const changed of [
    { ...values(), driver: 'relative' },
    { ...values(), driver_fingerprint: 'invalid' },
  ]) {
    const root = await directory(t)
    await assert.rejects(verifyProvenance(config(), changed, root), /bindings/)
  }
})

test('来源失败不启动业务观察；业务失败与后验失败同时保留', async (t) => {
  const root = await directory(t)
  const order = []
  const dependencies = {
    verifyProvenance: async () => { throw new Error('before failed') },
    createPacing: async () => ({ prepareSample: async () => { order.push('sample-window') },
      close: async () => { order.push('pacing-close') } }),
    prepareImportSamples: async () => { order.push('prepare') },
    observe: async () => { order.push('observe'); return async () => { order.push('stop') } },
    homepage: async () => { order.push('business'); throw new Error('business failed') },
  }
  await assert.rejects(measure(config(), { ...values(), output: path.join(root, 'before.json') }, 'cold', dependencies), /before failed/)
  assert.deepEqual(order, [])
  dependencies.verifyProvenance = async () => {
    order.push('before')
    return async () => { order.push('after'); throw new Error('after failed') }
  }
  await assert.rejects(measure(config(), { ...values(), output: path.join(root, 'after.json') }, 'cold', dependencies), (error) => {
    assert.ok(error instanceof AggregateError)
    assert.deepEqual(error.errors.map((value) => value.message), ['business failed', 'after failed'])
    return true
  })
  assert.deepEqual(order, ['before', 'sample-window', 'prepare', 'observe', 'business', 'stop', 'pacing-close', 'after'])
})

test('任务明细收尾失败后仍做后验，资源观察不包含来源检查', async (t) => {
  const order = []
  await assert.rejects(measure(config(), { ...values(), output: path.join(await directory(t), 'result.json') }, 'warm', {
    verifyProvenance: async () => { order.push('before'); return async () => { order.push('after') } },
    createPacing: async () => ({ prepareSample: async () => { order.push('sample-window') },
      close: async () => { order.push('pacing-close') } }),
    prepareImportSamples: async () => {},
    observe: async () => { order.push('observe'); return async () => { order.push('stop') } },
    homepage: async () => ({ finalize: async () => { order.push('finalize'); throw new Error('attempt failed') } }),
  }), /attempt failed/)
  assert.deepEqual(order, ['before', 'sample-window', 'observe', 'stop', 'finalize', 'pacing-close', 'after'])
})
