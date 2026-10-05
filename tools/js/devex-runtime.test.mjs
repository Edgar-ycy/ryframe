import test from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { once } from 'node:events'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { expand, hash, httpUrl, pointer } from './config.mjs'
import { quantile, summarize } from './load.mjs'
import { runRuntimeDriver } from './runtime.mjs'
import { metric } from './telemetry.mjs'
import { Session, operationCatalog, readBinaryResponse, readResponse } from './request.mjs'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')

test('契约 operation 使用真实前缀且不手写 URL', async () => {
  const catalog = await operationCatalog(process.cwd())
  assert.deepEqual(catalog.get('get_system_posts'), { method: 'GET', path: '/api/v1/system/posts' })
  assert.deepEqual(catalog.get('get_common_shell_settings'), { method: 'GET', path: '/api/v1/common/shell-settings' })
  assert.deepEqual(catalog.get('get_auth_tenants'), { method: 'GET', path: '/api/v1/auth/tenants' })
  assert.equal(catalog.size, 192)
})

test('下载按流计数，大 JSON 失败且不积累大文件内容', async () => {
  function response(chunks) {
    return { body: (async function* () { for (const chunk of chunks) yield chunk })() }
  }
  const block = Buffer.alloc(1024 * 1024)
  assert.deepEqual(await readResponse(response(Array(20).fill(block)), false), { bytes: 20 * 1024 * 1024 })
  await assert.rejects(readResponse(response(Array(17).fill(block)), true), /16 MiB/)
  assert.deepEqual(await readResponse(response([Buffer.from('{"n":'), Buffer.from('1}')]), true), { n: 1 })
})

test('用户导入模板二进制捕获受媒体类型和512KiB边界限制', async () => {
  const response = (chunks) => ({ body: (async function* () { for (const chunk of chunks) yield chunk })() })
  const content = Buffer.from('xlsx-template')
  const result = await readBinaryResponse(response([content]),
    'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet')
  assert.equal(result.bytes, content.length)
  assert.equal(Buffer.from(result.base64, 'base64').toString(), 'xlsx-template')
  assert.match(result.sha256, /^[a-f0-9]{64}$/)
  await assert.rejects(readBinaryResponse(response([]), 'application/octet-stream'), /为空/)
  await assert.rejects(readBinaryResponse(response([Buffer.alloc(512 * 1024 + 1)]),
    'application/octet-stream'), /512 KiB/)
  await assert.rejects(readBinaryResponse(response([content]), 'application/json'), /媒体类型/)
})

test('负载指纹不依赖 JSON 键顺序，变量保留 false 与 0', () => {
  assert.equal(hash({ b: 1, a: { y: 1, x: 2 } }), hash({ a: { x: 2, y: 1 }, b: 1 }))
  assert.deepEqual(expand({ count: '${count}', enabled: '${flag}' }, { count: 0, flag: false }), { count: 0, enabled: false })
  assert.equal(pointer({ 'a/b': { '~': 0 } }, '/a~1b/~0'), 0)
  assert.throws(() => pointer({}, '/missing'))
  assert.throws(() => expand('${missing}', {}))
  assert.throws(() => httpUrl('https://user:secret@example.test'))
})

test('失败请求独立统计，分位数使用 nearest rank', () => {
  assert.equal(quantile([1, 100, 2, 3, 4], .95), 100)
  const value = summarize([10, 20, 30], 2, 1000, [0, 20], [2, 4])
  assert.equal(value.failed, 2)
  assert.equal(value.completed, 3)
  assert.equal(value.throughput, 3)
  assert.equal(value.queue_p95_ms, 20)
  assert.equal(value.p99_ms, 30)
})

test('缺失、非数与负数指标失败，不把采集错误作为零', () => {
  assert.equal(metric('connections{a="x"} 1\nconnections{a="y"} 2', 'connections'), 3)
  assert.throws(() => metric('', 'connections'))
  assert.throws(() => metric('connections NaN', 'connections'))
  assert.throws(() => metric('connections -1', 'connections'))
})

test('运行时公开驱动保留输入输出、失败传播和配置前后像', async (context) => {
  const parent = path.join(ROOT, '.local-tests/node-unit')
  await mkdir(parent, { recursive: true })
  const root = await mkdtemp(path.join(parent, 'runtime-driver-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  const backend = path.join(root, 'backend')
  const frontend = path.join(root, 'frontend')
  const runnerFrontend = path.join(root, 'runner-frontend')
  const runtimeConfig = path.join(backend, '.local-tests/devex/runtime.json')
  await mkdir(path.dirname(runtimeConfig), { recursive: true })
  await mkdir(frontend)
  await mkdir(runnerFrontend)
  await writeFile(runtimeConfig, '{"fixed":true}\n')
  const environment = {
    RYFRAME_DEVEX_CACHE: 'warm',
    RYFRAME_DEVEX_RUNTIME_INPUT_SHA256: 'a'.repeat(64),
    RYFRAME_DEVEX_DRIVER_FINGERPRINT: `sha256:${'d'.repeat(64)}`,
    RYFRAME_DEVEX_RUNNER_FRONTEND_FINGERPRINT: `sha256:${'e'.repeat(64)}`,
  }
  const result = {
    measurement: {
      network_requests: 3,
      completed_cycles: 1,
      failed_cycles: 0,
      session_failures: 0,
      scenarios: { list: { completed: 1, failed: 0 } },
    },
    resources: { collector_failures: 0, cpu_seconds: 1 },
  }
  const argumentsFor = (output) => [
    '--suite', 'api', '--backend', backend, '--frontend', frontend,
    '--runner-frontend', runnerFrontend, '--output', output,
  ]
  const messages = []
  const runtimeValues = []
  const dependencies = {
    configuration: async () => ({ fixed: true }),
    measure: async (_config, values) => {
      runtimeValues.push(values)
      return result
    },
    report: (message) => messages.push(message),
  }

  const output = path.join(root, 'success.json')
  assert.equal(await runRuntimeDriver(argumentsFor(output), environment, dependencies), 0)
  assert.deepEqual(JSON.parse(await readFile(output, 'utf8')), {
    input_sha256: 'a'.repeat(64),
    cache_state: 'warm',
    ...result.measurement,
    ...result.resources,
  })
  assert.equal(runtimeValues[0].driver, ROOT)
  assert.equal(runtimeValues[0].driver_fingerprint, `sha256:${'d'.repeat(64)}`)
  assert.equal(runtimeValues[0].runner_frontend, runnerFrontend)
  assert.equal(runtimeValues[0].runner_frontend_fingerprint, `sha256:${'e'.repeat(64)}`)

  const failedEvidence = structuredClone(result)
  failedEvidence.measurement.completed_cycles = 0
  failedEvidence.measurement.failed_cycles = 1
  failedEvidence.measurement.scenarios.list = { completed: 0, failed: 1 }
  const failedEvidenceOutput = path.join(root, 'failed-evidence.json')
  assert.equal(await runRuntimeDriver(argumentsFor(failedEvidenceOutput), environment, {
    ...dependencies,
    measure: async () => failedEvidence,
  }), 1)
  assert.equal(JSON.parse(await readFile(failedEvidenceOutput, 'utf8')).failed_cycles, 1)

  const failureOutput = path.join(root, 'failure.json')
  assert.equal(await runRuntimeDriver(argumentsFor(failureOutput), environment, {
    ...dependencies,
    measure: async () => { throw new Error('fixture failure') },
  }), 1)
  await assert.rejects(readFile(failureOutput), /ENOENT/)

  const changedOutput = path.join(root, 'changed.json')
  assert.equal(await runRuntimeDriver(argumentsFor(changedOutput), environment, {
    ...dependencies,
    measure: async () => {
      await writeFile(runtimeConfig, '{"fixed":false}\n')
      return result
    },
  }), 1)
  await assert.rejects(readFile(changedOutput), /ENOENT/)

  const unboundOutput = path.join(root, 'unbound.json')
  const unboundEnvironment = { ...environment }
  delete unboundEnvironment.RYFRAME_DEVEX_DRIVER_FINGERPRINT
  assert.equal(await runRuntimeDriver(argumentsFor(unboundOutput), unboundEnvironment, dependencies), 1)
  await assert.rejects(readFile(unboundOutput), /ENOENT/)

  const unboundRunnerOutput = path.join(root, 'unbound-runner.json')
  const unboundRunnerEnvironment = { ...environment }
  delete unboundRunnerEnvironment.RYFRAME_DEVEX_RUNNER_FRONTEND_FINGERPRINT
  assert.equal(await runRuntimeDriver(
    argumentsFor(unboundRunnerOutput), unboundRunnerEnvironment, dependencies), 1)
  await assert.rejects(readFile(unboundRunnerOutput), /ENOENT/)

  const relativeRunner = argumentsFor(path.join(root, 'relative-runner.json'))
  relativeRunner[relativeRunner.indexOf('--runner-frontend') + 1] = 'relative'
  assert.equal(await runRuntimeDriver(relativeRunner, environment, dependencies), 2)

  assert.equal(await runRuntimeDriver([], environment, dependencies), 2)
  assert.equal(await runRuntimeDriver(['--unknown'], environment, dependencies), 2)
  assert.ok(messages.some((message) => message.includes('fixture failure')))
  assert.ok(messages.some((message) => message.includes('配置发生变化')))
  assert.ok(messages.some((message) => message.includes('driver')))
  assert.ok(messages.some((message) => message.includes('runner 前端')))
  assert.ok(messages.filter((message) => message.includes('参数无效')).length >= 2)
})

test('真实 HTTP 客户端携带租户、刷新 Cookie/CSRF，拒绝写轮询及错误终态', async () => {
  const calls = []
  const server = createServer((request, response) => {
    calls.push({ url: request.url, headers: request.headers })
    response.setHeader('Content-Type', 'application/json')
    if (request.url === '/csrf') {
      const challenge = request.headers.cookie?.includes('ryframe_refresh_token=refresh') ? 'bound' : 'anonymous'
      response.setHeader('Set-Cookie', `ryframe_csrf=${challenge}`)
      response.end(JSON.stringify({ code: 200, data: { csrf_token: challenge } }))
    } else if (request.url === '/login') {
      response.setHeader('Set-Cookie', 'ryframe_refresh_token=refresh; HttpOnly')
      response.end(JSON.stringify({ code: 200, data: { access_token: 'first' } }))
    } else if (['/refresh', '/logout'].includes(request.url)) {
      if (request.headers['x-csrf-token'] !== 'bound') {
        response.writeHead(403).end('{}')
        return
      }
      response.end(JSON.stringify({ code: 200, data: { access_token: 'second' } }))
    } else if (request.url === '/template') {
      response.setHeader('Content-Type', 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet')
      response.end('xlsx')
    } else response.end(JSON.stringify({ code: 200, data: { status: 'failed' } }))
  })
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  const previous = process.env.RYFRAME_DRIVER_TEST_PASSWORD
  process.env.RYFRAME_DRIVER_TEST_PASSWORD = 'test-fixture-only'
  try {
    const catalog = new Map([
      ['get_auth_csrf', { method: 'GET', path: '/csrf' }],
      ['post_auth_login', { method: 'POST', path: '/login' }],
      ['post_auth_refresh', { method: 'POST', path: '/refresh' }],
      ['post_auth_logout', { method: 'POST', path: '/logout' }],
      ['get_system_users_import_template', { method: 'GET', path: '/template' }],
      ['get_job', { method: 'GET', path: '/jobs' }],
    ])
    const session = new Session({ contract: { timeout_ms: 1000 },
      bindings: { api_url: `http://127.0.0.1:${server.address().port}`, frontend_url: 'http://localhost:4174' } }, catalog,
    { tenant_id: 'tenant-a', username: 'bench', password_env: 'RYFRAME_DRIVER_TEST_PASSWORD' })
    await session.login()
    await session.request({ operation: 'post_auth_refresh' })
    assert.equal(session.token, 'second')
    assert.equal(calls[1].headers['x-csrf-token'], 'anonymous')
    assert.equal(calls[3].headers['x-tenant-id'], 'tenant-a')
    assert.equal(calls[3].headers['x-csrf-token'], 'bound')
    assert.equal(calls[3].headers.origin, 'http://localhost:4174')
    assert.match(calls[3].headers.cookie, /ryframe_refresh_token=refresh/)
    const templateResult = await session.request({ operation: 'get_system_users_import_template', binary: true })
    assert.equal(Buffer.from(templateResult.base64, 'base64').toString(), 'xlsx')
    await assert.rejects(session.request({ operation: 'post_auth_refresh', poll: {} }), /只允许轮询/)
    await assert.rejects(session.request({ operation: 'get_job', binary: true }), /受限二进制/)
    await assert.rejects(session.workflow({ name: '任务', steps: [{ operation: 'get_job',
      poll: { pointer: '/data/status', equals: 'succeeded', failures: ['failed'] } }] }, {}), /后台状态失败/)
    await session.request({ operation: 'post_auth_logout' })
    assert.deepEqual(calls.map(call => call.url), ['/csrf', '/login', '/csrf', '/refresh', '/template', '/jobs', '/csrf', '/logout'])
    assert.equal(session.requests, 8)
  } finally {
    if (previous === undefined) delete process.env.RYFRAME_DRIVER_TEST_PASSWORD
    else process.env.RYFRAME_DRIVER_TEST_PASSWORD = previous
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
  }
})
