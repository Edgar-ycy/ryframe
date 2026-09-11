import { createHash } from 'node:crypto'
import { lstat, readFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import readline from 'node:readline'
import { hash, httpUrl } from './devex/config.mjs'
import { requestPacer } from './restore_reference_pacing.mjs'
import { verifyIdentitiesAt } from './restore_reference_existing.mjs'

const descriptorFields = [
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

function exactFields(value, fields, label) {
  if (
    !value ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    Object.keys(value).sort().join(',') !== [...fields].sort().join(',')
  )
    throw new Error(`${label}字段缺失或包含未登记内容`)
}

function validateDescriptor(value, label) {
  exactFields(value, ['bytes', 'path', 'sha256'], label)
  if (
    !Number.isSafeInteger(value.bytes) ||
    value.bytes <= 0 ||
    typeof value.path !== 'string' ||
    !path.isAbsolute(value.path) ||
    !/^[a-f0-9]{64}$/.test(value.sha256)
  )
    throw new Error(`${label}文件绑定无效`)
}

function validateHeader(lineage) {
  exactFields(
    lineage,
    [
      'format_version',
      'kind',
      'status',
      ...descriptorFields,
      'scopes',
      'scale',
      'tenants',
      'objects',
      'verification',
      'restore_qualified',
    ],
    '派生数据血缘',
  )
  for (const field of descriptorFields) validateDescriptor(lineage[field], `派生血缘 ${field}`)
  exactFields(
    lineage.scopes,
    ['origin_tenant_scope_id', 'current_source_scope_id', 'current_object_scope_id'],
    '派生血缘 scope',
  )
  exactFields(
    lineage.scale,
    [
      'records',
      'current_post_rows',
      'tenants',
      'post_samples',
      'business_objects',
      'verified_objects',
      'object_bytes',
    ],
    '派生血缘规模',
  )
  exactFields(
    lineage.verification,
    ['scope_id', 'api_url', 'frontend_url', 'request_interval_ms'],
    '派生来源验收运行参数',
  )
  exactFields(
    lineage.objects,
    ['business_objects', 'business_bytes', 'mapping_sha256', 'probe', 'verified_objects'],
    '派生来源对象投影',
  )
  if (
    lineage.format_version !== 1 ||
    lineage.kind !== 'restore-source-derived-dataset-lineage' ||
    lineage.status !== 'derived_dataset_verified' ||
    lineage.restore_qualified !== false ||
    !Array.isArray(lineage.tenants) ||
    lineage.tenants.length !== 11 ||
    !/^[a-z0-9][a-z0-9_-]{2,47}$/.test(lineage.scopes.origin_tenant_scope_id) ||
    !/^[a-z0-9][a-z0-9_-]{2,47}$/.test(lineage.verification.scope_id) ||
    lineage.scopes.origin_tenant_scope_id === lineage.verification.scope_id ||
    lineage.scopes.current_source_scope_id !== lineage.verification.scope_id ||
    lineage.scopes.current_object_scope_id !== lineage.verification.scope_id ||
    !Number.isSafeInteger(lineage.scale.records) ||
    !Number.isSafeInteger(lineage.scale.current_post_rows) ||
    !Number.isSafeInteger(lineage.scale.tenants) ||
    !Number.isSafeInteger(lineage.scale.post_samples) ||
    !Number.isSafeInteger(lineage.scale.business_objects) ||
    !Number.isSafeInteger(lineage.scale.verified_objects) ||
    !Number.isSafeInteger(lineage.scale.object_bytes) ||
    lineage.scale.tenants !== 11 ||
    lineage.scale.records < 100_000 ||
    lineage.scale.current_post_rows < lineage.scale.records ||
    lineage.scale.business_objects !== 256 ||
    lineage.scale.verified_objects !== 257 ||
    lineage.scale.object_bytes < 1024 ** 3 ||
    lineage.objects.business_objects !== lineage.scale.business_objects ||
    lineage.objects.business_bytes !== lineage.scale.object_bytes ||
    lineage.objects.verified_objects !== lineage.scale.verified_objects ||
    !/^[a-f0-9]{64}$/.test(lineage.objects.mapping_sha256)
  )
    throw new Error('派生数据血缘没有绑定正式恢复规模或当前来源')
}

function validateRuntime(lineage) {
  for (const value of [lineage.verification.api_url, lineage.verification.frontend_url]) {
    const url = httpUrl(value)
    if (!['127.0.0.1', '[::1]'].includes(url.hostname) || url.pathname !== '/')
      throw new Error('派生来源 API 与 CORS Origin 必须是明确 loopback 根地址')
  }
  requestPacer(lineage.verification.request_interval_ms)
  const probe = lineage.objects.probe
  exactFields(
    probe,
    ['bucket', 'source_key', 'target_key', 'bytes', 'sha256', 'metadata'],
    '派生来源唯一探针',
  )
  const origin = `${lineage.scopes.origin_tenant_scope_id}/`
  const current = `${lineage.scopes.current_object_scope_id}/`
  if (
    !['uploads', 'avatar', 'exports', 'imports', 'config-packages'].includes(probe.bucket) ||
    typeof probe.source_key !== 'string' ||
    !probe.source_key.startsWith(origin) ||
    typeof probe.target_key !== 'string' ||
    !probe.target_key.startsWith(current) ||
    !Number.isSafeInteger(probe.bytes) ||
    probe.bytes <= 0 ||
    probe.bytes > 4 * 1024 * 1024 ||
    !/^[a-f0-9]{64}$/.test(probe.sha256) ||
    !probe.metadata ||
    typeof probe.metadata !== 'object' ||
    Array.isArray(probe.metadata)
  )
    throw new Error('派生来源唯一探针没有绑定原始及当前对象 scope')
}

function validateTenant(identity, index, tenantIds, originScope) {
  exactFields(
    identity,
    ['tenant_id', 'database', 'username', 'password_env', 'records', 'posts', 'files'],
    '派生来源租户',
  )
  if (
    (index === 0) !== (identity.tenant_id === 'system') ||
    (index > 0 && identity.tenant_id !== `${originScope}-${String(index).padStart(2, '0')}`) ||
    !/^[a-z0-9][a-z0-9_-]{2,47}$/.test(identity.tenant_id) ||
    tenantIds.has(identity.tenant_id) ||
    typeof identity.database !== 'string' ||
    !identity.database ||
    typeof identity.username !== 'string' ||
    !identity.username ||
    !/^[A-Z][A-Z0-9_]*$/.test(identity.password_env) ||
    !Number.isSafeInteger(identity.records) ||
    identity.records < 0 ||
    !Array.isArray(identity.posts) ||
    identity.posts.length < 3 ||
    !Array.isArray(identity.files) ||
    !identity.files.length
  )
    throw new Error('派生来源租户身份、记录或样本无效')
  tenantIds.add(identity.tenant_id)
  for (const post of identity.posts) {
    exactFields(post, ['id', 'code', 'name'], '派生来源岗位样本')
    if (!/^[1-9][0-9]*$/.test(post.id) || !post.code || !post.name)
      throw new Error('派生来源岗位样本无效')
  }
  for (const file of identity.files) {
    exactFields(
      file,
      ['file_id', 'file_name', 'file_path', 'file_url', 'bytes', 'sha256'],
      '派生来源对象样本',
    )
    if (
      !/^[1-9][0-9]*$/.test(file.file_id) ||
      typeof file.file_name !== 'string' ||
      !file.file_name ||
      typeof file.file_path !== 'string' ||
      !file.file_path.startsWith(`${identity.tenant_id}/`) ||
      typeof file.file_url !== 'string' ||
      !file.file_url ||
      !Number.isSafeInteger(file.bytes) ||
      file.bytes <= 0 ||
      file.bytes > 4 * 1024 * 1024 ||
      !/^[a-f0-9]{64}$/.test(file.sha256)
    )
      throw new Error('派生来源对象样本无效')
  }
  return {
    records: identity.records,
    posts: identity.posts.length,
    files: identity.files.length,
    objectBytes: identity.files.reduce((sum, file) => sum + file.bytes, 0),
  }
}

export function validateSourceLineage(lineage) {
  validateHeader(lineage)
  validateRuntime(lineage)
  const totals = { records: 0, posts: 0, files: 0, objectBytes: 0 }
  const tenantIds = new Set()
  for (const [index, identity] of lineage.tenants.entries()) {
    const counts = validateTenant(
      identity,
      index,
      tenantIds,
      lineage.scopes.origin_tenant_scope_id,
    )
    for (const field of Object.keys(totals)) totals[field] += counts[field]
  }
  if (
    totals.records !== lineage.scale.records ||
    totals.posts !== lineage.scale.post_samples ||
    totals.files !== lineage.scale.business_objects ||
    totals.objectBytes !== lineage.scale.object_bytes
  )
    throw new Error('派生来源租户汇总与完整血缘规模不同')
}

export async function verifySourceExisting(backend, lineage) {
  validateSourceLineage(lineage)
  const original = hash(lineage)
  const subjects = []
  const counts = await verifyIdentitiesAt(
    backend,
    lineage.tenants,
    lineage.verification,
    18,
    lineage.verification.request_interval_ms,
    { clearBearerBeforeLogout: true, subjects },
  )
  if (hash(lineage) !== original) throw new Error('派生数据血缘在业务验证期间发生变化')
  return {
    format_version: 1,
    kind: 'restore-source-existing-verification',
    status: 'source_existing_data_verified',
    scope_id: lineage.verification.scope_id,
    origin_tenant_scope_id: lineage.scopes.origin_tenant_scope_id,
    lineage_sha256: original,
    actions: { business: 'read_only', objects: 'read_only', session: 'login_logout' },
    restore_success: false,
    subjects,
    ...counts,
  }
}

export function sourceExistingArguments(argv) {
  const names = new Set([
    '--backend-dir',
    '--lineage',
    '--run-dir',
    '--operation-id',
    '--source-generation-sha256',
    '--write',
  ])
  const args = new Map()
  for (let index = 0; index < argv.length; index++) {
    const name = argv[index]
    if (!names.has(name) || args.has(name)) throw new Error('派生来源验收参数未知或重复')
    const value = name === '--write' ? true : argv[++index]
    if (!value || (typeof value === 'string' && value.startsWith('--')))
      throw new Error('派生来源验收参数缺少值')
    args.set(name, value)
  }
  if (args.size !== names.size || args.get('--write') !== true)
    throw new Error('必须明确后端目录、派生数据血缘、运行身份和 --write')
  const result = {
    backend: path.resolve(args.get('--backend-dir')),
    lineagePath: path.resolve(args.get('--lineage')),
    runDirectory: args.get('--run-dir'),
    operationId: args.get('--operation-id'),
    sourceGenerationSha256: args.get('--source-generation-sha256'),
  }
  if (
    !path.isAbsolute(args.get('--backend-dir')) ||
    !path.isAbsolute(args.get('--lineage')) ||
    !path.isAbsolute(result.runDirectory) ||
    path.basename(result.runDirectory) !== 'verification' ||
    result.lineagePath !== path.join(path.dirname(result.runDirectory), 'dataset-lineage.json') ||
    !/^[a-f0-9]{32}$/.test(result.operationId) ||
    !/^[a-f0-9]{64}$/.test(result.sourceGenerationSha256)
  )
    throw new Error('来源验收必须绑定同代绝对目录、血缘、operation 与 start 摘要')
  return result
}

export async function waitForSourceStart(
  { runDirectory, operationId, sourceGenerationSha256 },
  stream = process.stdin,
) {
  const input = readline.createInterface({ input: stream, crlfDelay: Infinity })
  try {
    let authorization
    for await (const line of input) {
      if (authorization !== undefined) throw new Error('来源验收只接受一条启动授权')
      if (Buffer.byteLength(line) > 4096) throw new Error('来源验收启动授权过大')
      authorization = JSON.parse(line)
    }
    if (authorization === undefined)
      throw new Error('来源验收尚未取得持久进程身份授权，禁止请求服务')
    exactFields(
      authorization,
      ['operation', 'run_dir', 'operation_id', 'source_generation_sha256'],
      '来源验收启动授权',
    )
    if (
      authorization.operation !== 'start' ||
      authorization.run_dir !== runDirectory ||
      authorization.operation_id !== operationId ||
      authorization.source_generation_sha256 !== sourceGenerationSha256
    )
      throw new Error('来源验收启动授权不属于当前 start 与 operation')
  } finally {
    input.close()
  }
}

async function readSourceLineage(filename) {
  const before = await lstat(filename)
  if (!before.isFile() || before.isSymbolicLink() || before.size > 16 * 1024 * 1024)
    throw new Error('派生数据血缘必须是有界普通文件')
  const bytes = await readFile(filename)
  const after = await lstat(filename)
  if (bytes.length !== before.size || after.size !== before.size || after.mtimeMs !== before.mtimeMs)
    throw new Error('派生数据血缘在读取期间发生变化')
  return bytes
}

export async function verifySourceFile({ backend, lineagePath, sourceGenerationSha256 }) {
  const backendMetadata = await lstat(backend)
  if (!backendMetadata.isDirectory() || backendMetadata.isSymbolicLink())
    throw new Error('后端目录必须是非链接目录')
  const before = await readSourceLineage(lineagePath)
  const lineage = JSON.parse(before.toString('utf8'))
  const result = await verifySourceExisting(backend, lineage)
  const after = await readSourceLineage(lineagePath)
  if (!before.equals(after)) throw new Error('派生数据血缘在业务验收期间发生变化')
  return {
    ...result,
    source_generation_sha256: sourceGenerationSha256,
    lineage_file_sha256: createHash('sha256').update(before).digest('hex'),
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = sourceExistingArguments(process.argv.slice(2))
  await waitForSourceStart(args)
  const result = await verifySourceFile(args)
  process.stdout.write(JSON.stringify(result) + '\n')
}
