import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { Readable } from 'node:stream'
import { fileURLToPath, pathToFileURL } from 'node:url'
import {
  datasetArguments,
  datasetHelp,
  referenceClient,
  verifyExisting,
} from './restore_reference_existing.mjs'
import {
  sourceExistingArguments,
  validateSourceLineage,
  verifySourceExisting,
  waitForSourceStart,
} from './restore_source_existing.mjs'
import { Session } from './request.mjs'
import { hash } from './config.mjs'

const backend = fileURLToPath(new URL('../../', import.meta.url))
const bytes = Buffer.from('existing-object')
const scaleBytes = Buffer.alloc(4 * 1024 * 1024, 23)
const scaleSha256 = createHash('sha256').update(scaleBytes).digest('hex')

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

function sourceLineageFixture() {
  const descriptor = (name) => ({
    bytes: 1,
    path: fileURLToPath(new URL(`${name}.json`, import.meta.url)),
    sha256: 'a'.repeat(64),
  })
  let fileId = 1
  const tenants = Array.from({ length: 11 }, (_, index) => {
    const tenantId = index ? `frozen-source-${String(index).padStart(2, '0')}` : 'system'
    const fileCount = 23 + (index < 3 ? 1 : 0)
    return {
      tenant_id: tenantId,
      database: index === 10 ? 'dedicated-a' : index === 0 ? 'shared-control' : 'shared',
      username: index ? 'owner' : 'admin',
      password_env: 'TEST_PASSWORD',
      records: 9090 + (index < 10 ? 1 : 0),
      posts: Array.from({ length: 3 }, (_, post) => ({
        id: String(index * 3 + post + 1),
        code: `post-${index}-${post}`,
        name: `岗位${index}-${post}`,
      })),
      files: Array.from({ length: fileCount }, (_, file) => {
        const id = String(fileId++)
        return {
          file_id: id,
          file_name: `${id}.bin`,
          file_path: `${tenantId}/objects/${id}.bin`,
          file_url: `/common/file/download?path=${id}`,
          bytes: scaleBytes.length,
          sha256: scaleSha256,
        }
      }),
    }
  })
  const fields = [
    'source_registration',
    'seed_registration',
    'post_copy',
    'post_verify',
    'post_verify_evidence',
    'post_verify_target',
    'reference_plan',
    'dataset',
    'copy_stage_receipt',
    'copy_result',
    'ledger_head',
    'copy_plan',
    'current_image',
  ]
  return {
    format_version: 1,
    kind: 'restore-source-derived-dataset-lineage',
    status: 'derived_dataset_verified',
    ...Object.fromEntries(fields.map((field) => [field, descriptor(field)])),
    scopes: {
      origin_tenant_scope_id: 'frozen-source',
      current_source_scope_id: 'perf-seed',
      current_object_scope_id: 'perf-seed',
    },
    scale: {
      records: 100_000,
      current_post_rows: 100_000,
      tenants: 11,
      post_samples: 33,
      business_objects: 256,
      verified_objects: 257,
      object_bytes: 1024 ** 3,
    },
    tenants,
    objects: {
      business_objects: 256,
      business_bytes: 1024 ** 3,
      mapping_sha256: 'b'.repeat(64),
      probe: {
        bucket: 'uploads',
        source_key: 'frozen-source/system/probe.txt',
        target_key: 'perf-seed/system/probe.txt',
        bytes: 1,
        sha256: 'c'.repeat(64),
        metadata: { content_type: 'text/plain' },
      },
      verified_objects: 257,
    },
    verification: {
      scope_id: 'perf-seed',
      api_url: 'http://127.0.0.1:18210',
      frontend_url: 'http://127.0.0.1:4190',
      request_interval_ms: 1000,
    },
    restore_qualified: false,
  }
}

function transport(t, dataset, options = {}, SessionClass = Session) {
  const calls = []
  t.mock.method(SessionClass.prototype, 'login', async function () {
    calls.push({ operation: 'login', identity: this.identity, api: this.config.bindings.api_url })
    const tenantIndex = dataset.tenants.findIndex(
      (tenant) => tenant.tenant_id === this.identity.tenant_id,
    )
    const claims = Buffer.from(
      JSON.stringify({
        sub: String(9_007_199_254_740_993n + BigInt(tenantIndex)),
        tenant_id: this.identity.tenant_id,
        username: this.identity.username,
        token_type: 'access',
        user_authorization_version: 1,
      }),
    ).toString('base64url')
    this.token = `header.${claims}.signature`
    this.beforeRequest = async () => {}
  })
  t.mock.method(SessionClass.prototype, 'request', async function (step) {
    calls.push({
      operation: step.operation,
      identity: this.identity,
      api: this.config.bindings.api_url,
      authorization: this.token,
    })
    if (step.operation === 'post_auth_logout') {
      if (options.logoutFailure) throw new Error('logout failed')
      return {}
    }
    assert.equal(step.operation, 'get_system_posts_by_id')
    const post = dataset.tenants
      .find((value) => value.tenant_id === this.identity.tenant_id)
      .posts.find((value) => value.id === step.path.id)
    assert.ok(post)
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
    const body = options.wrongFile ? Buffer.from('wrong') : (options.fileBytes ?? bytes)
    if (options.streamFile)
      return new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(body)
            controller.close()
          },
        }),
      )
    return new Response(body)
  })
  return calls
}

test('数据帮助使用同一参数表，未知、重复和缺值仍失败关闭', () => {
  for (const flag of ['--help', '-h']) {
    assert.equal(datasetArguments([flag]).get('--help'), true)
    assert.equal(datasetArguments(['--plan', 'missing.json', flag]).get('--help'), true)
  }
  assert.match(datasetHelp(), /--verify-existing PATH/)
  assert.match(datasetHelp(), /--side source\|target/)
  for (const args of [
    ['--help', '--unknown'],
    ['--help', '-h'],
    ['--help', '--plan'],
  ])
    assert.throws(() => datasetArguments(args))
})

test('派生来源参数只接受绝对路径、唯一参数及显式写入授权', () => {
  const generation = fileURLToPath(new URL('g0001/', import.meta.url))
  const lineage = path.join(generation, 'dataset-lineage.json')
  const runDirectory = path.join(generation, 'verification')
  const operationId = 'd'.repeat(32)
  const sourceGenerationSha256 = 'e'.repeat(64)
  const valid = [
    '--backend-dir',
    backend,
    '--lineage',
    lineage,
    '--run-dir',
    runDirectory,
    '--operation-id',
    operationId,
    '--source-generation-sha256',
    sourceGenerationSha256,
    '--write',
  ]
  assert.deepEqual(
    sourceExistingArguments(valid),
    {
      backend: fileURLToPath(new URL('../../', import.meta.url)).replace(/[\\/]$/, ''),
      lineagePath: lineage,
      runDirectory,
      operationId,
      sourceGenerationSha256,
    },
  )
  for (const args of [
    valid.filter((value) => value !== '--write'),
    valid.with(1, '.'),
    valid.with(3, 'lineage.json'),
    valid.with(5, backend),
    valid.with(7, 'not-an-operation'),
    valid.with(9, 'bad-sha'),
    [...valid, '--lineage', lineage],
    [...valid, '--unknown', 'x'],
  ])
    assert.throws(() => sourceExistingArguments(args))
})

test('派生来源在任何服务请求前严格绑定生产者授权', async () => {
  const generation = fileURLToPath(new URL('g0001/', import.meta.url))
  const args = {
    runDirectory: path.join(generation, 'verification'),
    operationId: 'd'.repeat(32),
    sourceGenerationSha256: 'e'.repeat(64),
  }
  const authorization = {
    operation: 'start',
    run_dir: args.runDirectory,
    operation_id: args.operationId,
    source_generation_sha256: args.sourceGenerationSha256,
  }
  await waitForSourceStart(args, Readable.from(`${JSON.stringify(authorization)}\n`))
  for (const invalid of [
    '',
    '{}\n',
    `${JSON.stringify({ ...authorization, operation_id: 'f'.repeat(32) })}\n`,
    `${JSON.stringify({ ...authorization, extra: true })}\n`,
    `${JSON.stringify(authorization)}\n${JSON.stringify(authorization)}\n`,
    `${'x'.repeat(4097)}\n`,
  ])
    await assert.rejects(waitForSourceStart(args, Readable.from(invalid)))
})

test('来源授权窗口的 staging A→B→A 不改变已预载 ESM 与 OpenAPI', async (t) => {
  const parent = path.join(backend, '.local-tests', 'node-unit')
  await mkdir(parent, { recursive: true })
  const staging = await mkdtemp(path.join(parent, 'source-staging-'))
  t.after(() => rm(staging, { recursive: true, force: true }))
  const modules = [
    'tools/js/config.mjs',
    'tools/js/failure.mjs',
    'tools/js/pacing-model.mjs',
    'tools/js/request.mjs',
    'tools/js/restore_reference_existing.mjs',
    'tools/js/restore_reference_pacing.mjs',
    'tools/js/restore_source_existing.mjs',
  ]
  for (const relative of modules) {
    const target = path.join(staging, relative)
    await mkdir(path.dirname(target), { recursive: true })
    await copyFile(path.join(backend, relative), target)
  }
  await mkdir(path.join(staging, 'openapi'))
  await copyFile(path.join(backend, 'openapi/openapi.json'), path.join(staging, 'openapi/openapi.json'))
  const stagedFiles = []
  for (const relative of [...modules, 'openapi/openapi.json'].sort()) {
    const content = await readFile(path.join(staging, relative))
    stagedFiles.push({
      path: relative,
      origin: relative === 'openapi/openapi.json' ? 'execution' : 'coordinator',
      bytes: content.length,
      sha256: createHash('sha256').update(content).digest('hex'),
    })
  }
  await writeFile(
    path.join(staging, 'manifest.json'),
    JSON.stringify({
      format_version: 1,
      kind: 'restore-source-tool-staging',
      coordinator_source: { snapshot: 'A' },
      execution_sha: 'a'.repeat(40),
      entry: 'tools/js/restore_source_existing.mjs',
      contract: 'openapi/openapi.json',
      runtime_modules: modules,
      files: stagedFiles,
    }),
  )
  const lineage = sourceLineageFixture()
  const lineagePath = path.join(staging, 'dataset-lineage.json')
  await writeFile(lineagePath, JSON.stringify(lineage))
  const loaded = await import(
    `${pathToFileURL(path.join(staging, 'tools/js/restore_source_existing.mjs')).href}?test=${Date.now()}`
  )
  const stagedRequest = await import(
    pathToFileURL(path.join(staging, 'tools/js/request.mjs')).href
  )
  const args = {
    backend: staging,
    lineagePath,
    runDirectory: path.join(staging, 'verification'),
    operationId: 'd'.repeat(32),
    sourceGenerationSha256: 'e'.repeat(64),
  }
  const prepared = await loaded.prepareSourceFile(args)
  await loaded.waitForSourceStart(
    args,
    Readable.from(
      `${JSON.stringify({
        operation: 'start',
        run_dir: args.runDirectory,
        operation_id: args.operationId,
        source_generation_sha256: args.sourceGenerationSha256,
      })}\n`,
    ),
  )
  const entry = path.join(staging, 'tools/js/restore_source_existing.mjs')
  const reference = path.join(staging, 'tools/js/restore_reference_existing.mjs')
  const contract = path.join(staging, 'openapi/openapi.json')
  const original = await Promise.all([entry, reference, contract].map((file) => readFile(file)))
  await writeFile(entry, "throw new Error('staging B entry executed')\n")
  await writeFile(reference, "throw new Error('staging B dependency executed')\n")
  await writeFile(contract, '{"x-ryframe-api-prefix":{"value":"/api"},"paths":{}}\n')
  const calls = transport(
    t,
    lineage,
    { fileBytes: scaleBytes, streamFile: true },
    stagedRequest.Session,
  )
  let result
  try {
    result = await loaded.verifyPreparedSource(args, prepared)
  } finally {
    await Promise.all(
      [entry, reference, contract].map((file, index) => writeFile(file, original[index])),
    )
  }
  assert.equal(result.status, 'source_existing_data_verified')
  assert.equal(result.files, 256)
  assert.equal(calls.length, 311)
})

test('派生来源使用血缘中的原租户身份和当前端点完成全量业务读取', async (t) => {
  const lineage = sourceLineageFixture()
  const original = JSON.stringify(lineage)
  const calls = transport(t, lineage, { fileBytes: scaleBytes, streamFile: true })
  const result = await verifySourceExisting(backend, lineage)
  assert.deepEqual(result, {
    format_version: 1,
    kind: 'restore-source-existing-verification',
    status: 'source_existing_data_verified',
    scope_id: 'perf-seed',
    origin_tenant_scope_id: 'frozen-source',
    lineage_sha256: hash(lineage),
    actions: { business: 'read_only', objects: 'read_only', session: 'login_logout' },
    restore_success: false,
    subjects: lineage.tenants.map((tenant, index) => ({
      tenant_id: tenant.tenant_id,
      user_id: String(9_007_199_254_740_993n + BigInt(index)),
      user_authorization_version: 1,
    })),
    tenants: 11,
    posts: 33,
    files: 256,
  })
  assert.equal(JSON.stringify(lineage), original)
  assert.equal(calls.length, 311)
  assert.deepEqual(
    calls.filter((call) => call.operation === 'login').map((call) => call.identity.tenant_id),
    lineage.tenants.map((tenant) => tenant.tenant_id),
  )
  assert.ok(
    calls
      .filter((call) => call.operation === 'post_auth_logout')
      .every((call) => call.authorization === undefined),
  )
  for (const call of calls) {
    if (call.operation === 'download') {
      assert.equal(call.url.origin, 'http://127.0.0.1:18210')
      assert.ok(
        lineage.tenants.some(
          (tenant) => tenant.tenant_id === call.request.headers['X-Tenant-Id'],
        ),
      )
    } else {
      assert.equal(call.api, 'http://127.0.0.1:18210')
    }
  }
})

test('派生来源拒绝当前 scope 重写租户、汇总漂移和血缘外探针', () => {
  const original = sourceLineageFixture()
  validateSourceLineage(original)
  for (const change of [
    (value) => {
      value.tenants[1].tenant_id = 'perf-seed-01'
    },
    (value) => {
      value.tenants[1].records++
    },
    (value) => {
      value.objects.probe.target_key = 'other/system/probe.txt'
    },
    (value) => {
      value.current_image.extra = true
    },
  ]) {
    const candidate = structuredClone(original)
    change(candidate)
    assert.throws(() => validateSourceLineage(candidate))
  }
})

test('已有数据默认 target，显式 source；造数、缺值、重复及未知侧均失败关闭', () => {
  const base = ['--plan', 'plan.json', '--backend-dir', backend, '--preflight', 'preflight.json', '--write']
  assert.equal(datasetArguments(base).has('--side'), false)
  assert.equal(
    datasetArguments([
      '--plan',
      'plan.json',
      '--backend-dir',
      backend,
      '--verify-existing',
      'data.json',
      '--write',
    ]).get('--side'),
    'target',
  )
  assert.equal(
    datasetArguments([
      '--plan',
      'plan.json',
      '--backend-dir',
      backend,
      '--verify-existing',
      'data.json',
      '--side',
      'source',
      '--write',
    ]).get('--side'),
    'source',
  )
  for (const args of [
    [...base, '--side', 'source'],
    [...base, '--verify-existing', 'data.json'],
    base.filter((value) => value !== 'preflight.json' && value !== '--preflight'),
    [...base.filter((value) => value !== 'preflight.json' && value !== '--preflight'), '--verify-existing', 'data.json', '--side', 'other'],
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
