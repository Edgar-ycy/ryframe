import { mkdir, open, readFile, rename, lstat, unlink, readdir, link } from 'node:fs/promises'
import { createHash, randomUUID } from 'node:crypto'
import path from 'node:path'
import { localPath } from './identity-plan.mjs'
import { validateCompleteIdentityState } from './identity-apply.mjs'
import { validateVerifiedReceipt } from './identity-verify.mjs'

const encode = (value) => Buffer.from(JSON.stringify(value, null, 2) + '\n')
const sha256 = (value) => createHash('sha256').update(encode(value)).digest('hex')
const bytesSha256 = (value) => createHash('sha256').update(value).digest('hex')
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/

async function regularBytes(filename) {
  try {
    const metadata = await lstat(filename)
    if (metadata.isSymbolicLink() || !metadata.isFile()) throw new Error('身份证据必须是普通文件')
    return await readFile(filename)
  } catch (error) {
    if (error.code === 'ENOENT') return null
    throw error
  }
}

async function writeAtomic(filename, value) {
  try { if ((await lstat(filename)).isSymbolicLink()) throw new Error('账本不能是链接') }
  catch (error) { if (error.code !== 'ENOENT') throw error }
  const temporary = `${filename}.${randomUUID()}.tmp`
  const file = await open(temporary, 'wx')
  try { await file.writeFile(encode(value)); await file.sync() }
  finally { await file.close() }
  await rename(temporary, filename)
}

async function writeImmutable(filename, value) {
  const expected = encode(value), pending = `${filename}.pending`
  const current = await regularBytes(filename)
  if (current && !current.equals(expected)) throw new Error('已有身份发布证据不同，拒绝覆盖')
  let staged = await regularBytes(pending)
  if (staged && !staged.equals(expected)) throw new Error('身份发布暂存证据不同，拒绝覆盖')
  if (!current && !staged) {
    const file = await open(pending, 'wx')
    try { await file.writeFile(expected); await file.sync() }
    finally { await file.close() }
    staged = expected
  }
  if (!current) {
    try { await link(pending, filename) }
    catch (error) {
      const raced = await regularBytes(filename)
      if (error.code !== 'EEXIST' || !raced?.equals(expected)) throw error
    }
  }
  if (!(await regularBytes(filename))?.equals(expected)) throw new Error('身份发布证据落盘后不一致')
  if (await regularBytes(pending)) await unlink(pending)
}

function sameFileIdentity(left, right) {
  return left.dev === right.dev && left.ino === right.ino && left.birthtimeNs === right.birthtimeNs
}

async function releaseDirectory(lockName) {
  const directory = path.join(path.dirname(lockName), 'lock-releases')
  try { await mkdir(directory) }
  catch (error) { if (error.code !== 'EEXIST') throw error }
  const metadata = await lstat(directory)
  if (metadata.isSymbolicLink() || !metadata.isDirectory()) throw new Error('身份锁释放目录不是普通目录')
  return directory
}

function releaseNames(directory, releaseId) {
  return { tombstone: path.join(directory, `${releaseId}.lock`),
    marker: path.join(directory, `${releaseId}.released.json`) }
}

function lockValue(bytes) {
  let value
  try { value = JSON.parse(bytes.toString('utf8')) }
  catch { throw new Error('身份锁释放记录不可解析') }
  const fields = ['format_version', 'kind', 'pid', 'plan_sha256', 'owner_token']
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      Object.keys(value).sort().join() !== fields.sort().join() || value.format_version !== 1 ||
      value.kind !== 'devex-identity-run-lock' || !Number.isSafeInteger(value.pid) || value.pid <= 0 ||
      typeof value.plan_sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(value.plan_sha256) ||
      typeof value.owner_token !== 'string' || !uuid.test(value.owner_token) || !bytes.equals(encode(value)))
    throw new Error('身份锁释放记录未绑定规范 owner')
  return value
}

async function validateReleaseRecord(directory, releaseId) {
  const names = releaseNames(directory, releaseId)
  const tombstone = await regularBytes(names.tombstone), markerBytes = await regularBytes(names.marker)
  if (!tombstone || !markerBytes) throw new Error('身份锁存在未完成 release 记录，拒绝开放互斥')
  const lock = lockValue(tombstone)
  let marker
  try { marker = JSON.parse(markerBytes.toString('utf8')) }
  catch { throw new Error('身份锁 released marker 不可解析') }
  const fields = ['format_version', 'kind', 'release_id', 'tombstone_file', 'lock_sha256', 'lock']
  if (!marker || typeof marker !== 'object' || Array.isArray(marker) ||
      Object.keys(marker).sort().join() !== fields.sort().join() || marker.format_version !== 1 ||
      marker.kind !== 'devex-identity-run-lock-release' || marker.release_id !== releaseId ||
      marker.tombstone_file !== path.basename(names.tombstone) || marker.lock_sha256 !== bytesSha256(tombstone) ||
      !markerBytes.equals(encode(marker)) || !encode(marker.lock).equals(tombstone) ||
      JSON.stringify(marker.lock) !== JSON.stringify(lock)) throw new Error('身份锁 released marker 与墓碑不一致')
  if (!(await regularBytes(names.tombstone))?.equals(tombstone) ||
      !(await regularBytes(names.marker))?.equals(markerBytes)) throw new Error('身份锁 release 记录在核验期间变化')
  return { names, lock }
}

async function validateReleaseInventory(lockName) {
  const directory = await releaseDirectory(lockName), files = (await readdir(directory)).sort()
  const records = new Map(), owners = new Set()
  for (const name of files) {
    const match = /^([0-9a-f-]{36})\.(lock|released\.json)$/.exec(name)
    if (!match || !uuid.test(match[1])) throw new Error('身份锁释放目录存在未知或未完成证据，拒绝开放互斥')
    const record = records.get(match[1]) ?? new Set()
    if (record.has(match[2])) throw new Error('身份锁释放目录存在重复证据')
    record.add(match[2]); records.set(match[1], record)
  }
  for (const [releaseId, record] of records) {
    if (record.size !== 2 || !record.has('lock') || !record.has('released.json'))
      throw new Error('身份锁存在未完成 release 记录，拒绝开放互斥')
    const { lock } = await validateReleaseRecord(directory, releaseId)
    if (owners.has(lock.owner_token)) throw new Error('同一身份锁 owner 存在多个 released 记录')
    owners.add(lock.owner_token)
  }
  if (files.join('\0') !== (await readdir(directory)).sort().join('\0'))
    throw new Error('身份锁释放目录在核验期间变化')
  return directory
}

async function ownedPath(filename, owner, expected) {
  const handleIdentity = await owner.file.stat({ bigint: true })
  const before = await lstat(filename, { bigint: true }), bytes = await readFile(filename)
  const after = await lstat(filename, { bigint: true })
  if (before.isSymbolicLink() || !before.isFile() || !sameFileIdentity(owner.identity, handleIdentity) ||
      !sameFileIdentity(owner.identity, before) || !sameFileIdentity(before, after) || !bytes.equals(expected))
    throw new Error('身份运行锁已被替换，拒绝释放非本进程锁')
}

async function releaseOwnedLock(lockName, owner, expected = null, checkpoint = async () => {}) {
  let failure = null
  try {
    if (!expected) throw new Error('身份运行锁缺少 owner 字节')
    await ownedPath(lockName, owner, expected)
    await checkpoint('before-claim', { lock: lockName })
    const directory = await releaseDirectory(lockName), releaseId = randomUUID()
    const names = releaseNames(directory, releaseId)
    if (await regularBytes(names.tombstone) || await regularBytes(names.marker))
      throw new Error('身份锁随机 release 名称已存在')
    await rename(lockName, names.tombstone)
    await ownedPath(names.tombstone, owner, expected)
    await checkpoint('claimed', { lock: lockName, tombstone: names.tombstone, marker: names.marker })
    const marker = { format_version: 1, kind: 'devex-identity-run-lock-release', release_id: releaseId,
      tombstone_file: path.basename(names.tombstone), lock_sha256: bytesSha256(expected), lock: lockValue(expected) }
    await writeImmutable(names.marker, marker)
    await validateReleaseRecord(directory, releaseId)
    await checkpoint('released', { lock: lockName, tombstone: names.tombstone, marker: names.marker })
  } catch (error) { failure = error }
  try { await owner.file.close() }
  catch (error) { failure ??= error }
  if (failure) throw failure
}

async function acquireRunLock(lockName, plan, ownerToken) {
  if (typeof ownerToken !== 'string' ||
      !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(ownerToken))
    throw new Error('身份运行锁owner token无效')
  await validateReleaseInventory(lockName)
  const value = { format_version: 1, kind: 'devex-identity-run-lock', pid: process.pid,
    plan_sha256: plan.plan_sha256, owner_token: ownerToken }
  const expected = encode(value)
  const file = await open(lockName, 'wx')
  const owner = { file, identity: await file.stat({ bigint: true }), expected }
  try {
    await file.writeFile(expected)
    await file.sync()
    const current = await lstat(lockName, { bigint: true })
    if (current.isSymbolicLink() || !current.isFile() || !sameFileIdentity(owner.identity, current) ||
        !(await readFile(lockName)).equals(expected)) throw new Error('身份运行锁创建后身份不一致')
    await validateReleaseInventory(lockName)
    return owner
  } catch (error) {
    try { await releaseOwnedLock(lockName, owner, expected) }
    catch (cleanup) { throw new AggregateError([error, cleanup], '身份运行锁创建失败且无法安全清理') }
    throw error
  }
}

function currentState(state, plan, statuses) {
  const fields = ['format_version', 'plan_sha256', 'status', 'entries', 'roles', 'users',
    ...(['verifying', 'verified'].includes(state?.status) ? ['publication_nonce'] : []),
    ...(state?.status === 'verified' ? ['verified_receipt'] : [])]
  return state && typeof state === 'object' && !Array.isArray(state) &&
    Object.keys(state).sort().join() === fields.sort().join() && state.format_version === 1 &&
    state.plan_sha256 === plan.plan_sha256 && statuses.includes(state.status) && Array.isArray(state.entries) &&
    state.roles && typeof state.roles === 'object' && !Array.isArray(state.roles) &&
    state.users && typeof state.users === 'object' && !Array.isArray(state.users)
}

export function identityPublicationNames(plan, root) {
  const value = createHash('sha256').update(plan.plan_sha256).update('\0').update(path.resolve(root)).digest('hex')
    .slice(0, 32).split('')
  value[12] = '5'; value[16] = '8'
  const id = `${value.slice(0, 8).join('')}-${value.slice(8, 12).join('')}-${value.slice(12, 16).join('')}-` +
    `${value.slice(16, 20).join('')}-${value.slice(20).join('')}`
  return { intent: `identity-publication-${id}.json`, receipt: `verified-${id}.json` }
}

function preparedProjection(state) {
  return { format_version: state.format_version, plan_sha256: state.plan_sha256, status: 'prepared',
    entries: state.entries, roles: state.roles, users: state.users }
}

async function publicationIntent(plan, state, receipt, names, root) {
  await validateVerifiedReceipt(receipt, plan, state, root)
  return { format_version: 1, kind: 'devex-identity-verification-publication',
    plan_sha256: plan.plan_sha256, identity_state: path.resolve(root), publication_nonce: state.publication_nonce,
    ledger_sha256: sha256(preparedProjection(state)),
    receipt_file: names.receipt, receipt_sha256: sha256(receipt), receipt }
}

async function checkedIntent(value, plan, state, names, root) {
  const fields = ['format_version', 'kind', 'plan_sha256', 'identity_state', 'publication_nonce', 'ledger_sha256',
    'receipt_file', 'receipt_sha256', 'receipt']
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      Object.keys(value).sort().join() !== fields.sort().join() || value.format_version !== 1 ||
      value.kind !== 'devex-identity-verification-publication' || value.plan_sha256 !== plan.plan_sha256 ||
      !['verifying', 'verified'].includes(state.status) || value.identity_state !== path.resolve(root) ||
      value.publication_nonce !== state.publication_nonce ||
      value.ledger_sha256 !== sha256(preparedProjection(state)) || value.receipt_file !== names.receipt ||
      value.receipt_sha256 !== sha256(value.receipt)) throw new Error('身份发布checkpoint与账本或计划不一致')
  return validateVerifiedReceipt(value.receipt, plan, state, root)
}

async function inventory(root, names) {
  const allowed = new Set([names.intent, `${names.intent}.pending`, names.receipt, `${names.receipt}.pending`])
  const evidence = (await readdir(root)).filter((name) => {
    const normalized = name.toLowerCase()
    return normalized.startsWith('identity-publication-') || normalized.startsWith('verified-')
  })
  if (evidence.some((name) => !allowed.has(name))) throw new Error('存在计划外身份发布证据，拒绝猜测或覆盖')
  return { any: evidence.length > 0, intent: evidence.includes(names.intent) || evidence.includes(`${names.intent}.pending`),
    receipt: evidence.includes(names.receipt) || evidence.includes(`${names.receipt}.pending`) }
}

export async function identityLedger(plan, directory, mode, checkpoint = async () => {}, ownerToken = randomUUID(),
  releaseCheckpoint = async () => {}) {
  const root = await localPath(plan.environment.backend_dir, directory, false)
  await mkdir(root, { recursive: true })
  const lockName = path.join(root, 'lock'), lock = await acquireRunLock(lockName, plan, ownerToken)
  const filename = path.join(root, 'ledger.json'), names = identityPublicationNames(plan, root)
  let state, persisted
  try {
    if (mode === 'apply') {
      if ((await inventory(root, names)).any)
        throw new Error('已有验证发布checkpoint，必须通过verify安全收尾')
      try {
        state = JSON.parse(await readFile(await localPath(plan.environment.backend_dir, filename), 'utf8'))
        if (!currentState(state, plan, ['applying', 'prepared']))
          throw new Error('已有账本不属于当前计划或需要人工核对，不能重放')
      } catch (error) {
        if (error.code !== 'ENOENT') throw error
        state = { format_version: 1, plan_sha256: plan.plan_sha256, status: 'applying', entries: [], roles: {}, users: {} }
        await writeAtomic(filename, state)
      }
    } else {
      state = JSON.parse(await readFile(await localPath(plan.environment.backend_dir, filename), 'utf8'))
      if (!currentState(state, plan, ['prepared', 'verifying', 'verified']))
        throw new Error('账本不属于计划、未准备完成或需要人工核对未知提交')
      validateCompleteIdentityState(plan, state)
    }
    persisted = encode(state)
  } catch (error) {
    try { await releaseOwnedLock(lockName, lock, lock.expected, releaseCheckpoint) }
    catch (cleanup) { throw new AggregateError([error, cleanup], '身份账本打开失败且无法安全释放运行锁') }
    throw error
  }

  async function assertLedgerCurrent() {
    const current = await regularBytes(filename)
    if (!current?.equals(persisted)) throw new Error('身份账本在发布或写入期间被替换，保留现场')
  }

  async function persistState() {
    await assertLedgerCurrent()
    await writeAtomic(filename, state)
    persisted = encode(state)
    await assertLedgerCurrent()
  }

  async function resumeVerified() {
    await assertLedgerCurrent()
    validateCompleteIdentityState(plan, state)
    const found = await inventory(root, names)
    if (!found.intent) {
      if (found.receipt || state.status === 'verified') throw new Error('验证发布缺少确定性checkpoint，拒绝远程重验')
      return null
    }
    const intentName = path.join(root, names.intent), pending = `${intentName}.pending`
    const bytes = await regularBytes(intentName) ?? await regularBytes(pending)
    if (!bytes) throw new Error('身份发布checkpoint读取时消失，拒绝猜测')
    let intent
    try { intent = JSON.parse(bytes.toString('utf8')) }
    catch { throw new Error('身份发布checkpoint不可解析，拒绝覆盖') }
    const receipt = await checkedIntent(intent, plan, state, names, root)
    await writeImmutable(intentName, intent)
    await writeImmutable(path.join(root, names.receipt), receipt)
    await assertLedgerCurrent()
    if (state.status === 'verifying') {
      state.status = 'verified'
      state.verified_receipt = names.receipt
      await persistState()
    } else if (state.verified_receipt !== names.receipt) {
      throw new Error('verified账本没有指向确定性收据')
    }
    if (!currentState(state, plan, ['verified'])) throw new Error('验证发布收尾状态无效')
    validateCompleteIdentityState(plan, state, ['verified'])
    await assertLedgerCurrent()
    return receipt
  }

  return {
    root, state,
    persist: persistState,
    async requireApply() {
      if ((await inventory(root, names)).any) throw new Error('已有验证发布checkpoint，必须通过verify安全收尾')
    },
    async beginVerification() {
      if ((await inventory(root, names)).any) throw new Error('已有验证发布checkpoint，必须先按发布会话收尾')
      if (state.status === 'prepared') {
        state.status = 'verifying'
        state.publication_nonce = randomUUID()
        await persistState()
      } else if (state.status !== 'verifying') throw new Error('身份验证起点不是可恢复发布会话')
      validateCompleteIdentityState(plan, state, ['verifying'])
    },
    resumeVerified,
    async publishVerified(receipt) {
      if (state.status !== 'verifying' || (await inventory(root, names)).any)
        throw new Error('验证发布起点不是唯一verifying账本')
      validateCompleteIdentityState(plan, state, ['verifying'])
      const intent = await publicationIntent(plan, state, receipt, names, root)
      await writeImmutable(path.join(root, names.intent), intent)
      await checkpoint('receipt-boundary')
      await assertLedgerCurrent()
      await writeImmutable(path.join(root, names.receipt), receipt)
      await checkedIntent(intent, plan, state, names, root)
      await checkpoint('ledger-boundary')
      await assertLedgerCurrent()
      state.status = 'verified'
      state.verified_receipt = names.receipt
      await persistState()
      await checkpoint('outer-result-boundary')
      await assertLedgerCurrent()
    },
    async mutation(operation, slot, run, confirmed) {
      const entry = { operation, slot, phase: 'started' }
      state.entries.push(entry)
      await persistState()
      try {
        const result = await run()
        const fields = confirmed(result)
        Object.assign(entry, fields, { phase: 'confirmed' })
        await persistState()
        return result
      } catch {
        state.status = 'needs-reconciliation'
        await persistState()
        throw new Error('写操作结果未确认，账本需要人工核对；不重放或按同名接管')
      }
    },
    async close() {
      await releaseOwnedLock(lockName, lock, lock.expected, releaseCheckpoint)
    },
  }
}
