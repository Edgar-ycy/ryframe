import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { Readable } from 'node:stream'
import { hash } from '../devex/config.mjs'
import { Session } from '../devex/request.mjs'
import { verifyCloneExisting, validateCopyTarget } from '../restore_reference_existing.mjs'
import { cloneExistingArguments, verifyCloneFiles, waitForProducerStart } from '../devex_clone_existing.mjs'
import { fixture, sha256, transport } from './devex_clone_existing_fixture.mjs'

const backend = fileURLToPath(new URL('../../', import.meta.url))

test('复制目标校验保留原计划，读取原11租户33岗位256对象并独立报告复制证据', async (t) => {
  const { plan, dataset, binding } = fixture()
  const original = JSON.stringify({ plan, dataset, binding })
  const calls = transport(t, dataset)
  const result = await verifyCloneExisting(plan, backend, dataset, binding)
  assert.deepEqual(result, {
    format_version: 1,
    status: 'copy_existing_data_verified',
    side: 'copy_target',
    scope_id: 'seed-copy',
    plan_sha256: hash(plan),
    source_scope_id: 'original',
    target_binding_sha256: hash(binding),
    copy: binding.copy,
    actions: { business: 'read_only', objects: 'read_only', session: 'login_logout' },
    clone_verified: false,
    restore_success: false,
    tenants: 11,
    posts: 33,
    files: 256,
  })
  assert.equal(JSON.stringify({ plan, dataset, binding }), original)
  assert.equal(dataset.plan_sha256, hash(plan))
  assert.equal(plan.target.scope_id, 'restored')
  assert.equal(calls.length, 311)
  for (const call of calls) {
    if (call.operation === 'download') {
      assert.equal(call.url.origin, binding.target.api_url)
      assert.equal(call.url.searchParams.get('bucket'), 'uploads')
      assert.equal(call.request.redirect, 'error')
      const tenant = dataset.tenants.find(
        (value) => value.tenant_id === call.request.headers['X-Tenant-Id'],
      )
      assert.ok(tenant.files.some((file) => file.file_path === call.url.searchParams.get('path')))
    } else {
      assert.deepEqual(call.bindings, {
        api_url: binding.target.api_url,
        frontend_url: binding.target.frontend_url,
      })
      const tenant = dataset.tenants.find((value) => value.tenant_id === call.identity.tenant_id)
      assert.equal(call.identity.password_env, tenant.password_env)
      assert.equal(call.identity.username, tenant.username)
    }
  }
})

test('错误来源、复制摘要、作用域或非明确loopback目标在任何请求前拒绝', async (t) => {
  const value = fixture()
  const calls = transport(t, value.dataset)
  const changes = [
    (v) => {
      v.binding.source_plan_sha256 = '0'.repeat(64)
    },
    (v) => {
      v.binding.source_scope_id = 'wrong'
    },
    (v) => {
      v.binding.target.scope_id = 'original'
    },
    (v) => {
      v.binding.target.scope_id = 'UPPER'
    },
    (v) => {
      v.binding.target.scope_id = 'a'.repeat(49)
    },
    (v) => {
      v.binding.kind = 'restore-target'
    },
    (v) => {
      v.binding.format_version = 2
    },
    (v) => {
      v.binding.extra = true
    },
    (v) => {
      v.binding.target.extra = true
    },
    (v) => {
      v.binding.copy.extra = '0'.repeat(64)
    },
    (v) => {
      v.dataset.plan_sha256 = '0'.repeat(64)
    },
  ]
  for (const key of Object.keys(value.binding.copy)) {
    changes.push((v) => {
      delete v.binding.copy[key]
    })
    changes.push((v) => {
      v.binding.copy[key] = 'invalid'
    })
  }
  for (const key of ['api_url', 'frontend_url'])
    for (const address of [
      'http://example.com',
      'http://localhost:3300',
      'http://127.0.0.1:3300/path',
      'http://user:pass@127.0.0.1:3300',
      'http://127.0.0.1:3300?q=1',
    ])
      changes.push((v) => {
        v.binding.target[key] = address
      })
  for (const change of changes) {
    const candidate = structuredClone(value)
    change(candidate)
    await assert.rejects(
      verifyCloneExisting(candidate.plan, backend, candidate.dataset, candidate.binding),
    )
  }
  assert.equal(calls.length, 0)
})

test('复制scope格式与离线复制模型一致，允许已登记的下划线', () => {
  const { plan, binding } = fixture()
  binding.target.scope_id = 'seed_copy'
  assert.doesNotThrow(() => validateCopyTarget(plan, binding))
})

test('不能把原plan.target换为seed后重新计算binding来冒充原数据来源', async (t) => {
  const { plan, dataset, binding } = fixture()
  const calls = transport(t, dataset)
  plan.target = binding.target
  binding.source_plan_sha256 = hash(plan)
  await assert.rejects(
    verifyCloneExisting(plan, backend, dataset, binding),
    /没有绑定本次参考环境计划/,
  )
  assert.equal(calls.length, 0)
})

for (const [name, options] of [
  ['岗位错误', { wrongPost: true }],
  ['对象错误', { wrongFile: true }],
  ['注销失败', { logoutFailure: true }],
  ['读取与注销均失败', { wrongPost: true, logoutFailure: true }],
])
  test(`复制目标${name}保持失败且执行注销`, async (t) => {
    const { plan, dataset, binding } = fixture()
    const calls = transport(t, dataset, options)
    await assert.rejects(verifyCloneExisting(plan, backend, dataset, binding))
    assert.equal(calls.at(-1).operation, 'post_auth_logout')
    assert.equal(calls.filter((value) => value.operation === 'login').length, 1)
  })

test('验收中的对象变更拒绝成功，后续请求仍使用最初校验的目标快照', async (t) => {
  const { plan, dataset, binding } = fixture()
  const target = structuredClone(binding.target)
  const calls = transport(t, dataset, {
    onLogin() {
      binding.target.api_url = 'https://example.com'
    },
  })
  await assert.rejects(verifyCloneExisting(plan, backend, dataset, binding), /期间发生变化/)
  assert.ok(
    calls
      .filter((value) => value.bindings)
      .every((value) => value.bindings.api_url === target.api_url),
  )
  assert.ok(
    calls.filter((value) => value.url).every((value) => value.url.origin === target.api_url),
  )
})

test('租户密码环境缺失不能借管理员密码继续认证', async (t) => {
  const { plan, dataset, binding } = fixture()
  const originalLogin = Session.prototype.login
  const calls = transport(t, dataset)
  const missing = 'RYFRAME_COPY_MISSING_TENANT_PASSWORD'
  dataset.tenants[0].password_env = missing
  const previous = process.env[missing]
  delete process.env[missing]
  t.after(() => {
    if (previous !== undefined) process.env[missing] = previous
  })
  t.mock.method(Session.prototype, 'login', originalLogin)
  await assert.rejects(
    verifyCloneExisting(plan, backend, dataset, binding),
    /缺少显式身份或密码环境变量/,
  )
  assert.equal(calls.length, 0)
})

test('CLI要求全部明确参数及认证写入开关，拒绝重复、缺值和旧side选项', () => {
  const base = [
    '--plan',
    'p.json',
    '--dataset',
    'd.json',
    '--target-binding',
    'b.json',
    '--backend-dir',
    backend,
    '--run-dir',
    path.join(backend, '.local-tests/copy-run'),
    '--attempt',
    '2',
    '--write',
  ]
  assert.deepEqual(cloneExistingArguments(base), {
    planPath: path.resolve('p.json'),
    datasetPath: path.resolve('d.json'),
    bindingPath: path.resolve('b.json'),
    backend: path.resolve(backend),
    runDirectory: path.join(backend, '.local-tests/copy-run'),
    attempt: 2,
  })
  for (const invalid of [
    base.slice(0, -1),
    [...base, '--side', 'source'],
    [...base, '--write'],
    [...base, '--dataset'],
  ])
    assert.throws(() => cloneExistingArguments(invalid))
  for (const field of ['target', 'copy', 'source_scope_id']) {
    const { plan, binding } = fixture()
    delete binding[field]
    assert.throws(() => validateCopyTarget(plan, binding))
  }
})

test('Node读取在持久收据发布前不接受缺失或其他run的stdin授权', async () => {
  const args = { runDirectory: path.join(backend, '.local-tests/copy-run'), attempt: 2 }
  const good = { operation: 'start', run_dir: args.runDirectory, attempt: 2 }
  await waitForProducerStart(args, Readable.from([JSON.stringify(good) + '\n']))
  for (const value of [{ ...good, run_dir: backend }, { ...good, attempt: 1 }, { ...good, extra: true }])
    await assert.rejects(waitForProducerStart(args, Readable.from([JSON.stringify(value) + '\n'])))
  await assert.rejects(waitForProducerStart(args, Readable.from([])))
})

async function inputFixture(t) {
  const directoryRoot = path.join(backend, '.local-tests')
  await mkdir(directoryRoot, { recursive: true })
  const directory = await mkdtemp(path.join(directoryRoot, 'copy-existing-'))
  t.after(() => rm(directory, { recursive: true }))
  const value = fixture()
  const paths = {
    planPath: path.join(directory, 'plan.json'),
    datasetPath: path.join(directory, 'dataset.json'),
    bindingPath: path.join(directory, 'binding.json'),
    backend,
  }
  for (const [key, filename] of [
    ['plan', paths.planPath],
    ['dataset', paths.datasetPath],
    ['binding', paths.bindingPath],
  ])
    await writeFile(filename, JSON.stringify(value[key], null, 2) + '\n')
  return { ...value, paths }
}

test('内部CLI结果同时保留语义摘要与三个原始文件摘要', async (t) => {
  const { plan, dataset, binding, paths } = await inputFixture(t)
  transport(t, dataset)
  const result = await verifyCloneFiles(paths)
  assert.equal(result.plan_sha256, hash(plan))
  assert.equal(result.target_binding_sha256, hash(binding))
  assert.deepEqual(result.input_files, {
    plan_sha256: sha256(await readFile(paths.planPath)),
    dataset_sha256: sha256(await readFile(paths.datasetPath)),
    target_binding_sha256: sha256(await readFile(paths.bindingPath)),
  })
  assert.equal(result.posts, 33)
  assert.equal(result.files, 256)
})

for (const key of ['planPath', 'datasetPath', 'bindingPath'])
  test(`验收中${key}原始字节改变但JSON等价也拒绝成功`, async (t) => {
    const { dataset, paths } = await inputFixture(t)
    let changed = false
    transport(t, dataset, {
      async onLogin() {
        if (!changed) {
          changed = true
          await writeFile(paths[key], (await readFile(paths[key])) + ' ')
        }
      },
    })
    await assert.rejects(verifyCloneFiles(paths), /绑定文件在业务验证期间发生变化/)
  })
