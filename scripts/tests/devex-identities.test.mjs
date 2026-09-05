import test from 'node:test'
import assert from 'node:assert/strict'
import { cp, readFile, readdir, unlink, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import path from 'node:path'
import { executeIdentityPlan, main } from '../devex_prepare_identities.mjs'
import { createIdentityPlan, validateEnvironment, localPath } from '../devex/identity-plan.mjs'
import { activationSecret } from '../devex/identity-apply.mjs'
import { ordinaryContext, permissionIds } from '../devex/identity-verify.mjs'
import { identityLedger, identityPublicationNames } from '../devex/identity-ledger.mjs'
import { identityFixture, identityBackend } from './devex-identity-fixture.mjs'
import { selectWorkloadIdentities } from '../devex/selection.mjs'

const evidenceSha256 = (value) => createHash('sha256')
  .update(Buffer.from(JSON.stringify(value, null, 2) + '\n')).digest('hex')

test('离线计划固定200个普通用户和十租户分布，无网络或真实密码', async (t) => {
  const { environment, plan } = await identityFixture(t)
  assert.equal(plan.groups.length, 11)
  assert.equal(plan.groups[0].users.length, 100)
  assert.ok(plan.groups.slice(1).every((group) => group.users.length === 10))
  assert.equal(new Set(plan.groups.flatMap((group) => group.users.map((user) => user.client_address))).size, 200)
  assert.deepEqual(await createIdentityPlan(environment), plan)
  assert.ok(!JSON.stringify(plan).includes('Secret!Fixture'))
  for (const change of [
    (value) => { value.tenants[1].tenant_id = value.tenants[0].tenant_id },
    (value) => { value.quota.system_max_users = 100 },
    (value) => { value.passwords.system_env = 'raw-secret' },
    (value) => { value.api_url = 'https://example.test/' },
    (value) => { value.database.control.database = 'unsafe;drop' },
    (value) => { value.database.targets[0].connection.password_env = 'UNBOUND_DB_PASSWORD' },
    (value) => { value.system_admin.extra = 'secret' },
  ]) {
    const copy = structuredClone(environment); change(copy)
    assert.throws(() => validateEnvironment(copy))
  }
  await assert.rejects(localPath(environment.backend_dir, path.resolve('../outside.json'), false), /local-tests/)
})

test('fresh apply只发布prepared，部门阶段后独立verify才下载模板并发布verified', async (t) => {
  const { plan, directory } = await identityFixture(t), backend = identityBackend(plan)
  const state = path.join(directory, 'state')
  const applied = await executeIdentityPlan(plan, state, 'apply', backend.dependencies)
  assert.deepEqual(applied, { status: 'prepared', plan_sha256: plan.plan_sha256, entries: 622, roles: 11, users: 200 })
  assert.equal(backend.users.size, 200); assert.equal(backend.roles.size, 11)
  assert.equal(backend.events[0], 'inspect')
  assert.equal(backend.events.filter((operation) => operation === 'template').length, 0)
  assert.equal(JSON.parse(await readFile(path.join(state, 'ledger.json'))).status, 'prepared')
  assert.equal((await readdir(state)).filter((name) => name.startsWith('verified-')).length, 0)
  const beforePreparedRetry = backend.events.length
  assert.deepEqual(await executeIdentityPlan(plan, state, 'apply', backend.dependencies), applied)
  assert.ok(!backend.events.slice(beforePreparedRetry).some((operation) =>
    operation.startsWith('post_system_') || operation.startsWith('put_') || operation === 'post_auth_password_reset_complete'))
  const beforeVerify = backend.events.length
  const verified = await executeIdentityPlan(plan, state, 'verify', backend.dependencies)
  assert.deepEqual(verified, { status: 'verified', plan_sha256: plan.plan_sha256,
    users: 200, tenants: 10, message_audience: 10 })
  assert.equal(backend.closed(), 3)
  assert.ok(!backend.events.slice(beforeVerify).some((operation) => operation.startsWith('post_system_') || operation.startsWith('put_')))
  for (const file of (await readdir(state)).filter((name) => name.endsWith('.json'))) {
    const content = await readFile(path.join(state, file), 'utf8')
    assert.ok(!content.includes('secret-test-reset-token') && !content.includes('Secret!Fixture123'))
  }
  const ledger = JSON.parse(await readFile(path.join(state, 'ledger.json')))
  assert.equal(ledger.status, 'verified'); assert.equal(Object.keys(ledger.users).length, 200)
  const receiptName = (await readdir(state)).find((name) => name.startsWith('verified-'))
  const receipt = JSON.parse(await readFile(path.join(state, receiptName)))
  const selection = { contract: { identity_pools: receipt.identity_pools.contract,
    workloads: { jobs: [{ name: 'import', identity_pool: 'tenants' }] } },
  bindings: { identity_pools: receipt.identity_pools.bindings } }
  for (const concurrency of [10, 50, 100]) {
    assert.equal(new Set(selectWorkloadIdentities(selection, 'jobs', 'import', concurrency).map((identity) => identity.tenant_id)).size, 10)
  }
  await assert.rejects(executeIdentityPlan(plan, state, 'apply', backend.dependencies), /必须通过verify安全收尾/)
})

test('apply在已确认组边界中断后从确定性前缀继续且不重复写入', async (t) => {
  const { plan, directory } = await identityFixture(t), backend = identityBackend(plan, { interruptAfterSystem: true })
  const state = path.join(directory, 'interrupted')
  await assert.rejects(executeIdentityPlan(plan, state, 'apply', backend.dependencies), /clean interruption/)
  const interrupted = JSON.parse(await readFile(path.join(state, 'ledger.json')))
  assert.equal(interrupted.status, 'applying')
  assert.equal(interrupted.entries.length, 302)
  assert.equal(Object.keys(interrupted.roles).length, 1)
  assert.equal(Object.keys(interrupted.users).length, 100)
  const applied = await executeIdentityPlan(plan, state, 'apply', backend.dependencies)
  assert.equal(applied.status, 'prepared')
  assert.equal(backend.roles.size, 11)
  assert.equal(backend.users.size, 200)
  assert.equal(backend.events.filter((operation) => operation === 'post_system_roles').length, 11)
  assert.equal(backend.events.filter((operation) => operation === 'post_system_users').length, 200)
  assert.equal(backend.events.filter((operation) => operation === 'post_system_users_by_id_password_reset_requests').length, 200)
  assert.equal(backend.events.filter((operation) => operation === 'post_auth_password_reset_complete').length, 200)
})

test('verify只接受622项有序confirmed账本且不进入远程会话', async (t) => {
  const { plan, directory } = await identityFixture(t), backend = identityBackend(plan)
  const baseline = path.join(directory, 'strict-ledger-baseline')
  await executeIdentityPlan(plan, baseline, 'apply', backend.dependencies)
  const corruptions = [
    (ledger) => { ledger.entries.pop() },
    (ledger) => { ledger.entries[1].id = '9223372036854775807' },
    (ledger) => { [ledger.entries[2], ledger.entries[3]] = [ledger.entries[3], ledger.entries[2]] },
    (ledger) => { ledger.entries[301].phase = 'started' },
    (ledger) => { ledger.unexpected = true },
  ]
  for (const [index, corrupt] of corruptions.entries()) {
    const state = path.join(directory, `strict-ledger-${index}`)
    await cp(baseline, state, { recursive: true })
    const ledgerName = path.join(state, 'ledger.json')
    const ledger = JSON.parse(await readFile(ledgerName, 'utf8'))
    corrupt(ledger)
    await writeFile(ledgerName, JSON.stringify(ledger, null, 2) + '\n')
    const before = backend.events.length
    await assert.rejects(executeIdentityPlan(plan, state, 'verify', backend.dependencies), /账本/)
    assert.equal(backend.events.length, before)
    assert.ok(!(await readdir(state)).includes('lock'))
  }
})

test('写入已提交但响应未知时标记needs-reconciliation，禁止继续或同名接管', async (t) => {
  const { plan, directory } = await identityFixture(t), backend = identityBackend(plan, { unknownUserCreate: true })
  const state = path.join(directory, 'unknown')
  await assert.rejects(executeIdentityPlan(plan, state, 'apply', backend.dependencies), /人工核对/)
  const ledger = JSON.parse(await readFile(path.join(state, 'ledger.json')))
  assert.equal(ledger.status, 'needs-reconciliation')
  assert.equal(backend.users.size, 1)
  assert.equal(ledger.entries.at(-1).phase, 'started')
  const calls = backend.events.length
  await assert.rejects(executeIdentityPlan(plan, state, 'verify', backend.dependencies), /人工核对/)
  await assert.rejects(executeIdentityPlan(plan, state, 'apply', backend.dependencies), /已有账本/)
  assert.equal(backend.events.length, calls)
})

test('ownership失败先于会话，模板路径不一致只阻止最终verify', async (t) => {
  const { plan, directory } = await identityFixture(t), backend = identityBackend(plan)
  await assert.rejects(executeIdentityPlan(plan, path.join(directory, 'wrong'), 'apply', {
    ...backend.dependencies, python: async () => ({ scope_id: 'other', server_uuid: plan.environment.database.server_uuid }),
  }), /绑定错误/)
  assert.equal(backend.events.length, 0)
  const different = identityBackend(plan, { differentTemplate: true }), state = path.join(directory, 'templates')
  assert.equal((await executeIdentityPlan(plan, state, 'apply', different.dependencies)).status, 'prepared')
  await assert.rejects(executeIdentityPlan(plan, state, 'verify', different.dependencies), /部门路径不一致/)
  assert.equal(JSON.parse(await readFile(path.join(state, 'ledger.json'))).status, 'verifying')
  assert.equal((await readdir(state)).filter((name) => name.startsWith('verified-')).length, 0)
})

test('身份权限、激活fragment和来源漂移均严格失败关闭', async (t) => {
  const { plan, environment } = await identityFixture(t), group = plan.groups[0], user = group.users[0]
  const auth = { user: { id: '2', username: user.username, tenant_id: 'system' }, is_super_admin: false,
    roles: [group.role_code], permissions: group.permissions }
  assert.equal(ordinaryContext(auth, group, user, '2').user_id, '2')
  assert.throws(() => ordinaryContext({ ...auth, is_super_admin: true }, group, user, '2'))
  assert.throws(() => ordinaryContext({ ...auth, permissions: [...group.permissions, '*:*:*'] }, group, user, '2'))
  assert.throws(() => permissionIds([{ id: '1', code: 'wanted', status: '1', children: [] },
    { id: '2', code: 'wanted', status: '1', children: [] }], ['wanted']))
  const response = { data: { request_id: '3', expires_at: new Date(Date.now() + 60000).toISOString(),
    reset_url: '/reset-password#tenant_id=system&request_id=3&token=secret' } }
  assert.equal(activationSecret(response, environment, group).token, 'secret')
  assert.throws(() => activationSecret({ data: { ...response.data, reset_url: 'https://other.test/reset-password#token=secret' } }, environment, group))
  await writeFile(environment.environment_document.path, 'changed')
  await assert.rejects(createIdentityPlan(environment), /SHA/)
  await assert.rejects(main(['apply', '--plan', 'anything', '--state-dir', 'anything']), /--write/)
})

test('plan写入需要显式授权，拒绝时目录前后像不变', async (t) => {
  const { directory, environment } = await identityFixture(t)
  const manifest = path.join(directory, 'cli-environment.json')
  const output = path.join(directory, 'cli-plan.json')
  await writeFile(manifest, JSON.stringify(environment))
  const before = await readdir(directory)
  await assert.rejects(main(['plan', '--environment', manifest, '--output', output]), /--write/)
  assert.deepEqual(await readdir(directory), before)
  await main(['plan', '--environment', manifest, '--output', output, '--write'])
  const plan = JSON.parse(await readFile(output, 'utf8'))
  assert.equal(plan.plan_sha256, (await createIdentityPlan(environment)).plan_sha256)
})

test('等待证据保存失败时不将账本宣布verified，也不重复执行close', async (t) => {
  const { plan, directory } = await identityFixture(t), backend = identityBackend(plan)
  const state = path.join(directory, 'close-failure')
  let closes = 0
  await assert.rejects(executeIdentityPlan(plan, state, 'apply', { ...backend.dependencies,
    createPacing: async () => ({ createPreparationControls: async () => ({ beforeRequest: async () => {} }),
      close: async () => { closes++; throw new Error('evidence storage failure') } }),
  }), /evidence storage failure/)
  assert.equal(closes, 1)
  assert.equal(JSON.parse(await readFile(path.join(state, 'ledger.json'))).status, 'prepared')
  assert.equal((await readdir(state)).filter((name) => name.startsWith('verified-')).length, 0)
  const calls = backend.events.length
  assert.equal((await executeIdentityPlan(plan, state, 'apply', backend.dependencies)).status, 'prepared')
  assert.ok(!backend.events.slice(calls).some((operation) =>
    operation.startsWith('post_system_') || operation.startsWith('put_') || operation === 'post_auth_password_reset_complete'))
})

test('verify在receipt、ledger和外层结果边界崩溃后只收尾checkpoint且不重复远程会话', async (t) => {
  const { plan, directory } = await identityFixture(t), backend = identityBackend(plan)
  const baseline = path.join(directory, 'publication-baseline')
  await executeIdentityPlan(plan, baseline, 'apply', backend.dependencies)
  const summary = { status: 'verified', plan_sha256: plan.plan_sha256,
    users: 200, tenants: 10, message_audience: 10 }
  for (const boundary of ['receipt-boundary', 'ledger-boundary', 'outer-result-boundary']) {
    const state = path.join(directory, boundary)
    await cp(baseline, state, { recursive: true })
    let crashed = false
    const before = backend.events.length
    await assert.rejects(executeIdentityPlan(plan, state, 'verify', { ...backend.dependencies,
      publicationCheckpoint: async (stage) => {
        if (!crashed && stage === boundary) { crashed = true; throw new Error(`fixture ${boundary}`) }
      },
    }), new RegExp(`fixture ${boundary}`))
    const afterCrash = backend.events.length
    assert.ok(afterCrash > before)
    const interrupted = JSON.parse(await readFile(path.join(state, 'ledger.json')))
    assert.equal(interrupted.status, boundary === 'outer-result-boundary' ? 'verified' : 'verifying')

    assert.deepEqual(await executeIdentityPlan(plan, state, 'verify', backend.dependencies), summary)
    assert.equal(backend.events.length, afterCrash)
    const completed = JSON.parse(await readFile(path.join(state, 'ledger.json')))
    const files = await readdir(state)
    assert.equal(completed.status, 'verified')
    assert.equal(files.filter((name) => name.startsWith('identity-publication-') && name.endsWith('.json')).length, 1)
    assert.deepEqual(files.filter((name) => name.startsWith('verified-') && name.endsWith('.json')), [completed.verified_receipt])
    assert.ok(!files.some((name) => name.endsWith('.pending')))

    const completedFiles = files.slice().sort()
    assert.deepEqual(await executeIdentityPlan(plan, state, 'verify', backend.dependencies), summary)
    assert.equal(backend.events.length, afterCrash)
    assert.deepEqual((await readdir(state)).sort(), completedFiles)
  }
  const replaced = path.join(directory, 'ledger-replaced-at-publication')
  await cp(baseline, replaced, { recursive: true })
  const ledgerName = path.join(replaced, 'ledger.json')
  const malicious = '{"malicious":true}\n'
  await assert.rejects(executeIdentityPlan(plan, replaced, 'verify', { ...backend.dependencies,
    publicationCheckpoint: async (stage) => {
      if (stage === 'ledger-boundary') await writeFile(ledgerName, malicious)
    },
  }), /账本在发布或写入期间被替换/)
  assert.equal(await readFile(ledgerName, 'utf8'), malicious)
})

test('publication名称和intent绑定状态根，跨root复制不能跳过远程验证', async (t) => {
  const { plan, directory } = await identityFixture(t), backend = identityBackend(plan)
  const baseline = path.join(directory, 'root-binding-baseline')
  await executeIdentityPlan(plan, baseline, 'apply', backend.dependencies)
  const first = path.join(directory, 'root-binding-first')
  const second = path.join(directory, 'root-binding-second')
  await cp(baseline, first, { recursive: true })
  await cp(baseline, second, { recursive: true })
  await assert.rejects(executeIdentityPlan(plan, first, 'verify', { ...backend.dependencies,
    publicationCheckpoint: async (stage) => {
      if (stage === 'receipt-boundary') throw new Error('fixture root binding checkpoint')
    },
  }), /fixture root binding checkpoint/)
  const firstNames = identityPublicationNames(plan, first)
  const secondNames = identityPublicationNames(plan, second)
  assert.notDeepEqual(firstNames, secondNames)
  const intent = JSON.parse(await readFile(path.join(first, firstNames.intent), 'utf8'))
  intent.receipt_file = secondNames.receipt
  await writeFile(path.join(second, secondNames.intent), JSON.stringify(intent, null, 2) + '\n', { flag: 'wx' })
  const before = backend.events.length
  await assert.rejects(executeIdentityPlan(plan, second, 'verify', backend.dependencies), /checkpoint与账本或计划不一致/)
  assert.equal(backend.events.length, before)
  assert.equal(JSON.parse(await readFile(path.join(second, 'ledger.json'), 'utf8')).status, 'prepared')
})

test('resume逐字段重算完整receipt并重新核对模板实物', async (t) => {
  const { plan, directory } = await identityFixture(t), backend = identityBackend(plan)
  const state = path.join(directory, 'malicious-publication')
  await executeIdentityPlan(plan, state, 'apply', backend.dependencies)
  await assert.rejects(executeIdentityPlan(plan, state, 'verify', { ...backend.dependencies,
    publicationCheckpoint: async (stage) => {
      if (stage === 'receipt-boundary') throw new Error('fixture malicious checkpoint')
    },
  }), /fixture malicious checkpoint/)
  const names = identityPublicationNames(plan, state)
  const intentName = path.join(state, names.intent)
  const original = JSON.parse(await readFile(intentName, 'utf8'))
  const ledgerName = path.join(state, 'ledger.json')
  const verifyingLedger = JSON.parse(await readFile(ledgerName, 'utf8'))
  const preparedLedger = structuredClone(verifyingLedger)
  preparedLedger.status = 'prepared'
  delete preparedLedger.publication_nonce
  await writeFile(ledgerName, JSON.stringify(preparedLedger, null, 2) + '\n')
  const before = backend.events.length
  await assert.rejects(executeIdentityPlan(plan, state, 'verify', backend.dependencies), /checkpoint与账本或计划不一致/)
  assert.equal(backend.events.length, before)
  await writeFile(ledgerName, JSON.stringify(verifyingLedger, null, 2) + '\n')
  const forged = structuredClone(original)
  forged.receipt.identities[0].permissions_sha256 = '0'.repeat(64)
  forged.receipt_sha256 = evidenceSha256(forged.receipt)
  await writeFile(intentName, JSON.stringify(forged, null, 2) + '\n')
  await assert.rejects(executeIdentityPlan(plan, state, 'verify', backend.dependencies), /投影与完整账本不一致/)
  assert.equal(backend.events.length, before)
  await writeFile(intentName, JSON.stringify(original, null, 2) + '\n')
  await writeFile(original.receipt.templates[0].path, 'replaced-template')
  await assert.rejects(executeIdentityPlan(plan, state, 'verify', backend.dependencies), /模板实物与收据摘要不一致/)
  assert.equal(backend.events.length, before)
  assert.equal(JSON.parse(await readFile(ledgerName, 'utf8')).status, 'verifying')
})

test('Node锁只由owner token与同一文件身份释放，替换锁保持原样', async (t) => {
  const { plan, directory } = await identityFixture(t)
  const state = path.join(directory, 'lock-identity')
  const ownerToken = '01234567-89ab-4cde-8fab-0123456789ab'
  const ledger = await identityLedger(plan, state, 'apply', async () => {}, ownerToken)
  const lockName = path.join(state, 'lock')
  const owner = JSON.parse(await readFile(lockName, 'utf8'))
  assert.deepEqual(Object.keys(owner).sort(), ['format_version', 'kind', 'owner_token', 'pid', 'plan_sha256'])
  assert.equal(owner.kind, 'devex-identity-run-lock')
  assert.equal(owner.owner_token, ownerToken)
  await unlink(lockName)
  const replacement = '{"replacement":true}\n'
  await writeFile(lockName, replacement, { flag: 'wx' })
  await assert.rejects(ledger.close(), /运行锁已被替换/)
  assert.equal(await readFile(lockName, 'utf8'), replacement)
})

test('Node锁在最终核验后的替换不会被旧owner删除且未发布墓碑持续阻止互斥', async (t) => {
  const { plan, directory } = await identityFixture(t)
  const state = path.join(directory, 'lock-before-claim-race')
  const ownerToken = '11234567-89ab-4cde-8fab-0123456789ab'
  const replacement = '{"replacement":"new-owner"}\n'
  const ledger = await identityLedger(plan, state, 'apply', async () => {}, ownerToken,
    async (stage, paths) => {
      if (stage !== 'before-claim') return
      await unlink(paths.lock)
      await writeFile(paths.lock, replacement, { flag: 'wx' })
    })
  await assert.rejects(ledger.close(), /运行锁已被替换/)
  const releases = path.join(state, 'lock-releases')
  const files = await readdir(releases)
  assert.equal(files.filter((name) => name.endsWith('.lock')).length, 1)
  assert.equal(files.filter((name) => name.endsWith('.released.json')).length, 0)
  assert.equal(await readFile(path.join(releases, files.find((name) => name.endsWith('.lock'))), 'utf8'), replacement)
  await assert.rejects(identityLedger(plan, state, 'apply', async () => {},
    '21234567-89ab-4cde-8fab-0123456789ab'), /未完成 release/)
})

test('Node锁claim后的canonical replacement保持原位且正常release可再次获取', async (t) => {
  const { plan, directory } = await identityFixture(t)
  const normal = path.join(directory, 'lock-normal-release')
  const first = await identityLedger(plan, normal, 'apply', async () => {},
    '31234567-89ab-4cde-8fab-0123456789ab')
  await first.close()
  await assert.rejects(readFile(path.join(normal, 'lock')), /ENOENT/)
  assert.deepEqual((await readdir(path.join(normal, 'lock-releases'))).map((name) => name.split('.').slice(1).join('.')).sort(),
    ['lock', 'released.json'])
  const second = await identityLedger(plan, normal, 'apply', async () => {},
    '41234567-89ab-4cde-8fab-0123456789ab')
  await second.close()

  const raced = path.join(directory, 'lock-after-claim-race')
  const replacement = '{"replacement":"new-owner"}\n'
  const ledger = await identityLedger(plan, raced, 'apply', async () => {},
    '51234567-89ab-4cde-8fab-0123456789ab', async (stage, paths) => {
      if (stage === 'claimed') await writeFile(paths.lock, replacement, { flag: 'wx' })
    })
  await ledger.close()
  assert.equal(await readFile(path.join(raced, 'lock'), 'utf8'), replacement)
  const releases = await readdir(path.join(raced, 'lock-releases'))
  assert.equal(releases.filter((name) => name.endsWith('.lock')).length, 1)
  assert.equal(releases.filter((name) => name.endsWith('.released.json')).length, 1)
  await assert.rejects(identityLedger(plan, raced, 'apply', async () => {},
    '61234567-89ab-4cde-8fab-0123456789ab'), /EEXIST/)
})

test('计划外receipt在apply和verify重进前均失败关闭且不被覆盖', async (t) => {
  const { plan, directory } = await identityFixture(t), backend = identityBackend(plan)
  const state = path.join(directory, 'ambiguous-publication')
  await executeIdentityPlan(plan, state, 'apply', backend.dependencies)
  const orphan = path.join(state, 'Verified-00000000-0000-0000-0000-000000000000.json')
  await writeFile(orphan, '{"unknown":true}\n', { flag: 'wx' })
  const before = backend.events.length
  await assert.rejects(executeIdentityPlan(plan, state, 'verify', backend.dependencies), /计划外身份发布证据/)
  await assert.rejects(executeIdentityPlan(plan, state, 'apply', backend.dependencies), /计划外身份发布证据/)
  assert.equal(backend.events.length, before)
  assert.equal(await readFile(orphan, 'utf8'), '{"unknown":true}\n')
  assert.equal(JSON.parse(await readFile(path.join(state, 'ledger.json'))).status, 'prepared')
})
