import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { hash } from '../devex/config.mjs'
import { importSampleModel, importSpecification, prepareImportSamples, sampleReader } from '../devex/import-samples.mjs'
import { Session } from '../devex/request.mjs'
import { measure } from '../devex/measure.mjs'
import { identityPool } from './devex-selection-fixture.mjs'

const digest = (value) => createHash('sha256').update(value).digest('hex')
function config() {
  const pool = identityPool(10, { tenants: 10 })
  return { contract: { cycles: 2, identity_pools: { users: pool.contract },
    workloads: { jobs: [{ name: 'import', identity_pool: 'users', job: { kind: 'import' } }] },
    import_samples: { template_sha256: 'a'.repeat(64), model_sha256: hash(importSampleModel) } },
  bindings: { identity_pools: { users: pool.binding },
    import_samples: { template: path.resolve('template.xlsx'), python: path.resolve('python') } } }
}

async function produced(_executable, request) {
  await mkdir(request.directory)
  const files = []
  for (let worker = 0; worker < request.concurrency; worker++) {
    for (let cycle = 0; cycle < request.cycles; cycle++) {
      const filename = `${worker}-${cycle}.xlsx`
      const username = `dv${request.namespace}${worker.toString(16).padStart(2, '0')}${cycle.toString(16).padStart(4, '0')}`
      await writeFile(path.join(request.directory, filename), username)
      files.push({ sample: `${worker}:${cycle}`, filename, username, sha256: digest(username) })
    }
  }
  return { format_version: 1, namespace: request.namespace, template_sha256: request.template_sha256,
    model: importSampleModel, model_sha256: request.model_sha256, concurrency: request.concurrency,
    cycles: request.cycles, files }
}

test('连续sample拥有独立文件/用户名清单，环境变量数量不增长', async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'devex-import-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const before = { ...process.env }, names = new Set()
  for (const arm of ['a-1', 'b-1', 'b-2', 'a-2']) {
    const directory = path.join(root, arm)
    const reader = await prepareImportSamples(config(), 'jobs', 2, directory, produced)
    const receipt = JSON.parse(await readFile(path.join(directory, 'import-preparation.json')))
    assert.equal(receipt.success, true)
    assert.ok(receipt.duration_ms >= 0)
    assert.equal(receipt.manifest.files.length, 4)
    for (const file of receipt.manifest.files) {
      assert.equal(names.has(file.username), false)
      names.add(file.username)
      assert.equal((await reader(file.sample)).toString(), file.username)
    }
    await assert.rejects(reader('100:0'), /未登记/)
    const selected = receipt.manifest.files[0]
    await writeFile(path.join(receipt.directory, selected.filename), 'changed')
    await assert.rejects(reader(selected.sample), /SHA已变化/)
  }
  assert.deepEqual({ ...process.env }, before)
  assert.equal(names.size, 16)
})

test('manifest拒绝跨目录文件、重复样本和伪造当前namespace', async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'devex-manifest-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const request = { ...importSpecification(config()), directory: path.join(root, 'files'),
    namespace: 'b'.repeat(20), concurrency: 2, cycles: 2 }
  const manifest = await produced('unused', request)
  await assert.rejects(sampleReader(request.directory, { ...manifest, namespace: 'c'.repeat(20) }, request), /不一致/)
  const escaped = structuredClone(manifest)
  escaped.files[0].filename = '../outside.xlsx'
  await assert.rejects(sampleReader(request.directory, escaped, request), /越界样本/)
  const duplicate = structuredClone(manifest)
  duplicate.files[1] = duplicate.files[0]
  await assert.rejects(sampleReader(request.directory, duplicate, request), /重复/)
})

test('multipart恰选sample或path_env，固定上传仍保留且未登记sample不发送HTTP', async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'devex-multipart-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const old = process.env.RYFRAME_TEST_MULTIPART
  process.env.RYFRAME_TEST_MULTIPART = path.join(root, 'fixed.xlsx')
  t.after(() => { if (old === undefined) delete process.env.RYFRAME_TEST_MULTIPART; else process.env.RYFRAME_TEST_MULTIPART = old })
  await writeFile(process.env.RYFRAME_TEST_MULTIPART, 'fixed')
  const session = new Session({}, {}, {}, { multipartSample: async (sample) => {
    if (sample !== '0:0') throw new Error('未登记')
    return Buffer.from('sample')
  } })
  assert.equal((await session.multipartContent({ sample: '0:0' })).toString(), 'sample')
  assert.equal((await session.multipartContent({ path_env: 'RYFRAME_TEST_MULTIPART' })).toString(), 'fixed')
  await assert.rejects(session.multipartContent({}), /必须且只能/)
  await assert.rejects(session.multipartContent({ sample: '0:0', path_env: 'RYFRAME_TEST_MULTIPART' }), /必须且只能/)
  await assert.rejects(session.multipartContent({ sample: '../outside' }), /未登记/)
  assert.equal(session.requests, 0)
})

test('准备失败保存收据，不生成批量环境变量或进入业务测量', async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'devex-prepare-failure-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await assert.rejects(prepareImportSamples(config(), 'jobs', 2, root, async () => { throw new Error('fixture failure') }), /fixture failure/)
  const receipt = JSON.parse(await readFile(path.join(root, 'import-preparation.json')))
  assert.equal(receipt.success, false)
  assert.equal(receipt.failure, 'import_sample_preparation_failed')
  assert.throws(() => importSpecification({ contract: {}, bindings: {} }), /固定全有效模型/)
})

test('准备和catalog在资源观察/HTTP计时前完成，attempt收据在资源观察结束后完成', async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'devex-measure-import-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const previous = process.env.RYFRAME_DEVEX_CONCURRENCY
  process.env.RYFRAME_DEVEX_CONCURRENCY = '10'
  t.after(() => { if (previous === undefined) delete process.env.RYFRAME_DEVEX_CONCURRENCY; else process.env.RYFRAME_DEVEX_CONCURRENCY = previous })
  const order = [], reader = async () => Buffer.from('sample')
  const dependencies = {
    verifyProvenance: async () => { order.push('verify-before'); return async () => { order.push('verify-after') } },
    createPacing: async () => ({ prepareSample: async () => { order.push('sample-window') },
      close: async () => { order.push('pacing-close') } }),
    prepareImportSamples: async () => { order.push('prepare'); return reader },
    operationCatalog: async () => { order.push('catalog'); return {} },
    observe: async () => { order.push('observe'); return async () => { order.push('stop'); return {} } },
    load: async (...args) => { order.push('http'); assert.equal(args[5].multipartSample, reader)
      return { finalize: async () => { order.push('attempts') } } },
  }
  const output = path.join(root, 'first.json')
  await measure(config(), { suite: 'jobs', output }, 'warm', dependencies)
  assert.deepEqual(order, ['verify-before', 'sample-window', 'prepare', 'catalog', 'observe', 'http', 'stop', 'attempts', 'pacing-close', 'verify-after'])
  const selection = JSON.parse(await readFile(path.join(output + '.artifacts', 'identity-selection.json')))
  assert.equal(selection.selections[0].count, 10)
  assert.equal(Object.keys(selection.selections[0].tenant_distribution).length, 10)
  assert.equal(JSON.stringify(selection).includes('password_env'), false)
  order.length = 0
  await assert.rejects(measure(config(), { suite: 'jobs', output: path.join(root, 'failed.json') }, 'warm', {
    ...dependencies, prepareImportSamples: async () => { throw new Error('prepare failed') },
  }), /prepare failed/)
  assert.deepEqual(order, ['verify-before', 'sample-window', 'pacing-close', 'verify-after'])
  order.length = 0
  await assert.rejects(measure({ ...config(), bindings: { ...config().bindings, identity_pools: { users: [] } } },
    { suite: 'jobs', output: path.join(root, 'invalid.json') }, 'warm', dependencies), /独立用户/)
  assert.deepEqual(order, [])
})
