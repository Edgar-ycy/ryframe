import { appendFile, mkdir, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import { operationCatalog } from './devex/request.mjs'
import { hash, httpUrl } from './devex/config.mjs'
import { requestPacer } from './restore_reference_pacing.mjs'
import {
  datasetArguments,
  datasetHelp,
  referenceClient,
  verifyExisting,
} from './restore_reference_existing.mjs'
import { strictScalarJsonObject } from './private_json_protocol.mjs'
import { pythonInvocation } from './python_process.mjs'

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex')
const protocolKey = 'RYFRAME_XTASK_RECOVERY_DATASET_PREPARE'
const protocolPrefix = 'RYFRAME_XTASK_RECOVERY_DATASET_PREPARE'
const protocolKind = 'ryframe-xtask-recovery-dataset-prepare'

export class DatasetPrepareProtocolError extends Error {}

function exactProtocol(value, fields) {
  if (JSON.stringify(Object.keys(value).sort()) !== JSON.stringify(fields.slice().sort()))
    throw new DatasetPrepareProtocolError('dataset-prepare 私有协议字段不完整或含未知字段')
}

function protocolPath(value, label, optional = false) {
  if (optional && value === null) return null
  if (typeof value !== 'string' || !value || !path.isAbsolute(value) || /[\n\r\0]/u.test(value))
    throw new DatasetPrepareProtocolError(`${label}必须是无换行和 NUL 的非空绝对路径`)
  return value
}

export function privateDatasetArguments(argv = process.argv.slice(2), environment = process.env) {
  if (argv.length)
    throw new DatasetPrepareProtocolError('dataset-prepare 是私有实现，不接受命令行参数')
  const unknown = Object.keys(environment).filter(
    (name) => name.startsWith(protocolPrefix) && name !== protocolKey,
  )
  if (unknown.length)
    throw new DatasetPrepareProtocolError(
      `dataset-prepare 私有环境含未知字段：${unknown.sort().join(',')}`,
    )
  const protocol = strictScalarJsonObject(
    environment[protocolKey],
    DatasetPrepareProtocolError,
    'dataset-prepare 私有协议',
  )
  exactProtocol(protocol, [
    'backend_dir',
    'format_version',
    'kind',
    'plan',
    'preflight',
    'side',
    'verify_existing',
    'write',
  ])
  if (
    protocol.format_version !== 1 ||
    protocol.kind !== protocolKind ||
    protocol.write !== true
  )
    throw new DatasetPrepareProtocolError('dataset-prepare 私有协议版本、类型或写入授权无效')
  const backend = protocolPath(protocol.backend_dir, 'backend_dir')
  const plan = protocolPath(protocol.plan, 'plan')
  const preflight = protocolPath(protocol.preflight, 'preflight', true)
  const existing = protocolPath(protocol.verify_existing, 'verify_existing', true)
  if ((preflight === null) === (existing === null))
    throw new DatasetPrepareProtocolError('dataset-prepare 私有协议必须选择唯一执行模式')
  if (preflight !== null && protocol.side !== 'source')
    throw new DatasetPrepareProtocolError('数据准备必须固定 source 侧')
  if (existing !== null && !['source', 'target'].includes(protocol.side))
    throw new DatasetPrepareProtocolError('已有数据核验侧无效')
  const arguments_ = ['--plan', plan, '--backend-dir', backend]
  if (preflight !== null) arguments_.push('--preflight', preflight)
  else arguments_.push('--verify-existing', existing, '--side', protocol.side)
  arguments_.push('--write')
  return arguments_
}

export function verifyDatasetPreflight(plan, preflight) {
  if (
    preflight?.format_version !== 1 ||
    preflight.kind !== 'restore-reference-dataset-preflight' ||
    preflight.plan_sha256 !== hash(plan) ||
    preflight.side !== 'source' ||
    preflight.scope_id !== plan.source.scope_id ||
    preflight.actions?.business !== 'empty_source_verified'
  )
    throw new Error('数据准备预检收据未绑定当前计划、source 侧或空源检查')
}

export function datasetSpecification(plan) {
  const settings = plan.dataset
  if (!/^[a-z][a-z0-9_-]{2,31}$/.test(plan.id)) throw new Error('数据集ID必须为3到32位小写安全标识')
  if (!settings || settings.tenant_targets?.length !== 10)
    throw new Error('参考数据集必须包含10个普通租户及已有system租户')
  requestPacer(settings.request_interval_ms)
  if (
    !Number.isSafeInteger(settings.api_validation_posts) ||
    settings.api_validation_posts < 1 ||
    settings.api_validation_posts > 10 ||
    !Number.isSafeInteger(settings.post_batch_rows) ||
    settings.post_batch_rows < 100 ||
    settings.post_batch_rows > 2_000
  )
    throw new Error('参考数据准备必须声明每租户 API 验证记录数和100到2000行的岗位批次大小')
  if (
    !Number.isSafeInteger(settings.records) ||
    settings.records < 100_000 ||
    settings.records > 1_000_000
  )
    throw new Error('参考业务记录必须为100000到1000000条')
  if (
    !Number.isSafeInteger(settings.object_count) ||
    settings.object_count < 11 ||
    settings.object_count > 1024 ||
    !Number.isSafeInteger(settings.object_bytes) ||
    settings.object_bytes < 1024 ||
    settings.object_bytes > 4 * 1024 * 1024 ||
    settings.object_count * settings.object_bytes < 1024 ** 3
  )
    throw new Error('参考对象必须合计至少1GiB，每个不超过4MiB，共11到1024个')
  if (
    settings.admin?.tenant_id !== 'system' ||
    !settings.admin.username ||
    !/^[A-Z][A-Z0-9_]+$/.test(settings.admin.password_env) ||
    !/^[A-Z][A-Z0-9_]+$/.test(settings.owner_password_env)
  )
    throw new Error('必须明确system管理员与租户管理员密码环境变量')
  const targets = new Map(plan.source.databases.map((database) => [database.key, database]))
  if (
    settings.tenant_targets.some((key) => !targets.has(key)) ||
    !settings.tenant_targets.some((key) => targets.get(key).mode === 'dedicated') ||
    !settings.tenant_targets.some((key) => targets.get(key).mode === 'shared')
  )
    throw new Error('参考租户必须覆盖已登记的共享和独立目标')
  for (const key of settings.tenant_targets)
    if (
      targets.get(key).mode === 'dedicated' &&
      settings.tenant_targets.filter((value) => value === key).length !== 1
    )
      throw new Error('独立目标只能分配一个普通租户')
  for (const value of [plan.source.api_url, plan.source.frontend_url]) {
    const url = httpUrl(value)
    if (!['127.0.0.1', '[::1]'].includes(url.hostname) || url.pathname !== '/')
      throw new Error('参考API与前端必须使用明确loopback根地址')
  }
  return settings
}

function tenantRecordCount(records, index, tenants = 11) {
  return Math.floor(records / tenants) + (index < records % tenants ? 1 : 0)
}

export function postBatchRows(plan, identities) {
  const settings = datasetSpecification(plan)
  const rows = []
  const samples = []
  for (const [identityIndex, identity] of identities.entries()) {
    const count = tenantRecordCount(settings.records, identityIndex)
    for (let index = settings.api_validation_posts; index < count; index++) {
      const row = {
        tenant_id: identity.tenant_id,
        index,
        code: `${plan.id}-${index}`,
        name: `恢复样本${index}`,
        sort: index % 1000,
      }
      if (index === settings.api_validation_posts || index === count - 1)
        samples.push({ identityIndex, position: rows.length, row })
      rows.push(row)
    }
  }
  if (rows.length !== settings.records - identities.length * settings.api_validation_posts)
    throw new Error('岗位批次行数与当前计划规模不一致')
  return { rows, samples }
}

export function postBatchSamples(samples, result) {
  if (
    result?.kind !== 'restore-reference-post-batch' ||
    !/^[1-9][0-9]*$/.test(result.first_id) ||
    !Number.isSafeInteger(result.rows) ||
    result.rows < 1 ||
    !Number.isSafeInteger(result.batch_rows) ||
    !Number.isSafeInteger(result.batches) ||
    !/^[1-9][0-9]*$/.test(result.last_id) ||
    BigInt(result.last_id) !== BigInt(result.first_id) + BigInt(result.rows - 1)
  )
    throw new Error('岗位批次结果不完整')
  const first = BigInt(result.first_id)
  return samples.map(({ identityIndex, position, row }) => ({
    identityIndex,
    id: (first + BigInt(position)).toString(),
    code: row.code,
    name: row.name,
  }))
}

function preparePostBatch(plan, backend, planPath, input) {
  const python = process.env.RYFRAME_PYTHON?.trim() || 'python'
  const invocation = pythonInvocation({
    script: path.join(backend, 'scripts/restore_reference_post_batch.py'),
    arguments: ['--backend-dir', backend, '--plan', planPath, '--input', input],
  })
  let output
  try {
    output = execFileSync(
      python,
      invocation.argv,
      {
        encoding: 'utf8',
        env: invocation.env,
        windowsHide: true,
        timeout: 900_000,
        maxBuffer: 1024 * 1024,
      },
    )
  } catch (error) {
    throw new Error('岗位批次准备失败；当前阶段含有未知写入，禁止重放', { cause: error })
  }
  try {
    return JSON.parse(output)
  } catch {
    throw new Error('岗位批次准备未返回唯一 JSON 收据')
  }
}

export function sampleContent(id, index, bytes) {
  const output = Buffer.alloc(bytes)
  for (let offset = 0, block = 0; offset < bytes; block++) {
    const line = Buffer.from(`${sha256(`${id}:${index}:${block}`)}\n`)
    offset += line.copy(output, offset, 0, Math.min(line.length, bytes - offset))
  }
  return output
}

export async function seedPosts(session, id, tenant, count, record) {
  const samples = []
  for (let index = 0; index < count; index++) {
    if (index > 0 && index % 100 === 0) await session.request({ operation: 'post_auth_refresh' })
    const code = `${id}-${index}`
    const response = await session.request({
      operation: 'post_system_posts',
      body: { name: `恢复样本${index}`, code, sort: index % 1000 },
    })
    const post = response?.data
    if (!/^[1-9][0-9]*$/.test(post?.id) || post.code !== code)
      throw new Error('岗位创建结果没有对应当前数据集')
    await record({ kind: 'post', tenant_id: tenant, id: post.id, code })
    if (index < 2 || index === count - 1) samples.push({ id: post.id, code, name: post.name })
  }
  const page = await session.request({
    operation: 'get_system_posts',
    query: { code: id, page: 1, page_size: 1 },
  })
  if (page?.data?.total !== count) throw new Error('岗位数量与本次精确编码前缀不一致')
  return samples
}

async function tenants(plan, admin, catalog) {
  const settings = plan.dataset
  const ownerPassword = process.env[settings.owner_password_env]
  if (!ownerPassword) throw new Error('缺少租户管理员密码环境变量')
  const capabilities = await admin.request({ operation: 'get_platform_capabilities' })
  if (JSON.stringify(capabilities?.data) !== '[]') throw new Error('参考套餐要求当前空能力目录')
  const product = await admin.request({
    operation: 'post_platform_product_plans',
    body: { key: plan.id, name: `恢复参考${plan.id}` },
  })
  const version = await admin.request({
    operation: 'post_platform_product_plans_by_plan_id_versions',
    path: { plan_id: product.data.id },
    body: { name: '恢复参考版本', capabilities: [] },
  })
  await admin.request({
    operation: 'post_platform_product_plans_by_plan_id_versions_by_version_publish',
    path: { plan_id: product.data.id, version: version.data.version },
  })
  const result = [
    {
      tenant_id: 'system',
      username: settings.admin.username,
      password_env: settings.admin.password_env,
      session: admin,
    },
  ]
  for (let index = 0; index < 10; index++) {
    const tenant_id = `${plan.source.scope_id}-${String(index + 1).padStart(2, '0')}`
    await admin.request({
      operation: 'post_platform_tenants',
      body: {
        tenant_id,
        name: `恢复参考租户${index + 1}`,
        admin_username: 'owner',
        admin_password: ownerPassword,
        data_target_key: settings.tenant_targets[index],
        plan_version_id: version.data.id,
        max_users: 20,
        max_roles: 10,
        max_storage_mb: 512,
        max_requests_per_min: 60_000,
      },
    })
    const identity = { tenant_id, username: 'owner', password_env: settings.owner_password_env }
    const session = referenceClient(plan, catalog, identity)
    await session.login()
    result.push({ ...identity, session })
  }
  return result
}

export async function prepareDataset(plan, backend, planPath) {
  const settings = datasetSpecification(plan)
  const directory = path.join(plan.work_dir, 'dataset')
  await mkdir(directory, { recursive: false })
  const catalog = await operationCatalog(backend)
  const admin = referenceClient(plan, catalog, settings.admin)
  await admin.login()
  const identities = await tenants(plan, admin, catalog)
  const record = async (value) =>
    appendFile(path.join(directory, 'created.ndjson'), JSON.stringify(value) + '\n')
  const result = {
    format_version: 1,
    plan_sha256: hash(plan),
    source_scope_id: plan.source.scope_id,
    started_at: new Date().toISOString(),
    records: 0,
    object_bytes: 0,
    tenants: [],
    request_interval_ms: settings.request_interval_ms,
    post_concurrency: 11,
  }
  let failure
  const prepared = await Promise.allSettled(
    identities.map(async (identity, index) => {
      try {
        await identity.session.request({ operation: 'post_auth_refresh' })
        const count = tenantRecordCount(settings.records, index)
        const posts = await seedPosts(
          identity.session,
          plan.id,
          identity.tenant_id,
          settings.api_validation_posts,
          async (row) => {
            await record(row)
            if (failure) throw new Error('其他参考租户准备失败，停止新增请求')
          },
        )
        result.tenants[index] = {
          tenant_id: identity.tenant_id,
          username: identity.username,
          password_env: identity.password_env,
          records: count,
          api_records: settings.api_validation_posts,
          batch_records: count - settings.api_validation_posts,
          posts,
          files: [],
        }
        result.records += count
      } catch (error) {
        failure ??= error
        throw error
      }
    }),
  )
  if (prepared.some((item) => item.status === 'rejected')) throw failure
  const batch = postBatchRows(plan, identities)
  const batchInput = path.join(directory, 'post-batch.ndjson')
  await writeFile(batchInput, batch.rows.map((row) => JSON.stringify(row)).join('\n') + '\n', { flag: 'wx' })
  const batchResult = preparePostBatch(plan, backend, planPath, batchInput)
  if (
    batchResult.plan_sha256 !== hash(plan) ||
    batchResult.rows !== batch.rows.length ||
    batchResult.batch_rows !== settings.post_batch_rows ||
    batchResult.batches !== Math.ceil(batch.rows.length / settings.post_batch_rows) ||
    batchResult.input_sha256 !== sha256(await readFile(batchInput))
  )
    throw new Error('岗位批次结果未绑定当前计划或精确输入行数')
  for (const sample of postBatchSamples(batch.samples, batchResult))
    result.tenants[sample.identityIndex].posts.push({
      id: sample.id,
      code: sample.code,
      name: sample.name,
    })
  result.records = settings.records
  result.post_batch = batchResult
  const sample = path.join(directory, 'upload.txt')
  for (const identity of identities)
    await identity.session.request({ operation: 'post_auth_refresh' })
  process.env.RYFRAME_REFERENCE_UPLOAD = sample
  for (let index = 0; index < settings.object_count; index++) {
    const content = sampleContent(plan.id, index, settings.object_bytes)
    await writeFile(sample, content)
    const owner = index % identities.length
    const response = await identities[owner].session.request({
      operation: 'post_common_upload',
      multipart: [
        {
          path_env: 'RYFRAME_REFERENCE_UPLOAD',
          field: 'file',
          type: 'text/plain',
          filename: `${plan.id}-${index}.txt`,
        },
      ],
    })
    if (response?.data?.length !== 1 || !response.data[0].file_path || !response.data[0].file_id)
      throw new Error('上传没有返回唯一文件登记')
    const file = { ...response.data[0], bytes: content.length, sha256: sha256(content) }
    await record({ kind: 'file', tenant_id: identities[owner].tenant_id, ...file })
    result.tenants[owner].files.push(file)
    result.object_bytes += content.length
  }
  result.completed_at = new Date().toISOString()
  await writeFile(path.join(directory, 'result.json'), JSON.stringify(result, null, 2) + '\n', {
    flag: 'wx',
  })
  return result
}

export async function main(argv = process.argv.slice(2)) {
  const args = datasetArguments(argv)
  if (args.has('--help')) {
    process.stdout.write(datasetHelp())
    return
  }
  const side = args.get('--side')
  const plan = JSON.parse(await readFile(args.get('--plan'), 'utf8'))
  const backend = path.resolve(args.get('--backend-dir'))
  const originalPreflight = args.has('--preflight')
    ? await readFile(args.get('--preflight'))
    : null
  if (originalPreflight) {
    verifyDatasetPreflight(plan, JSON.parse(originalPreflight.toString('utf8')))
  } else {
    const protocolEnvironment = Object.fromEntries(
      Object.entries(process.env).filter(([name]) => !name.startsWith('RYFRAME_XTASK_RECOVERY_')),
    )
    protocolEnvironment.RYFRAME_XTASK_RECOVERY_REFERENCE = JSON.stringify({
      format_version: 1,
      kind: 'ryframe-xtask-recovery-reference',
      request: {
        backend_dir: backend,
        format_version: 1,
        operation: 'check-existing',
        plan: path.resolve(args.get('--plan')),
        side,
        write: false,
      },
    })
    const invocation = pythonInvocation({
      script: path.join(backend, 'scripts/restore_reference.py'),
      environment: protocolEnvironment,
    })
    const verified = JSON.parse(
      execFileSync(
        process.env.RYFRAME_PYTHON || 'python',
        invocation.argv,
        {
          encoding: 'utf8',
          env: invocation.env,
          windowsHide: true,
          timeout: 60_000,
        },
      ),
    )
    if (
      verified.plan_sha256 !== hash(plan) ||
      verified.side !== side ||
      verified.scope_id !== plan[side].scope_id
    )
      throw new Error('参考计划或检查侧在ownership检查期间发生变化')
  }
  const original = args.has('--verify-existing')
    ? await readFile(args.get('--verify-existing'))
    : null
  const result = args.has('--verify-existing')
    ? await verifyExisting(plan, backend, JSON.parse(original.toString('utf8')), side)
    : await prepareDataset(plan, backend, args.get('--plan'))
  if (original) {
    if (!original.equals(await readFile(args.get('--verify-existing'))))
      throw new Error('原数据收据在验证期间发生变化')
    result.dataset_sha256 = sha256(original)
  }
  if (originalPreflight) {
    if (!originalPreflight.equals(await readFile(args.get('--preflight'))))
      throw new Error('数据准备预检收据在执行期间发生变化')
  }
  if (hash(JSON.parse(await readFile(args.get('--plan'), 'utf8'))) !== hash(plan))
    throw new Error('参考计划在验证期间发生变化')
  process.stdout.write(JSON.stringify(result) + '\n')
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const privateEnvironment = { ...process.env }
  delete process.env[protocolKey]
  try {
    await main(privateDatasetArguments(process.argv.slice(2), privateEnvironment))
  } catch (error) {
    if (error instanceof DatasetPrepareProtocolError) {
      process.stderr.write('dataset_prepare_protocol_error：数据准备私有协议无效。\n')
      process.exitCode = 2
    } else {
      throw error
    }
  }
}
