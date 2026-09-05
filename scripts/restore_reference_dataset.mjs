import { appendFile, mkdir, readFile, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import { operationCatalog } from './devex/request.mjs'
import { hash, httpUrl } from './devex/config.mjs'
import { requestPacer } from './restore_reference_pacing.mjs'
import { datasetArguments, referenceClient, verifyExisting } from './restore_reference_existing.mjs'

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex')

export function datasetSpecification(plan) {
  const settings = plan.dataset
  if (!/^[a-z][a-z0-9_-]{2,31}$/.test(plan.id)) throw new Error('数据集ID必须为3到32位小写安全标识')
  if (!settings || settings.tenant_targets?.length !== 10)
    throw new Error('参考数据集必须包含10个普通租户及已有system租户')
  requestPacer(settings.request_interval_ms)
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

export async function prepareDataset(plan, backend) {
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
        const count = Math.floor(settings.records / 11) + (index < settings.records % 11 ? 1 : 0)
        const posts = await seedPosts(
          identity.session,
          plan.id,
          identity.tenant_id,
          count,
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

async function main() {
  const args = datasetArguments(process.argv.slice(2))
  const plan = JSON.parse(await readFile(args.get('--plan'), 'utf8'))
  const backend = path.resolve(args.get('--backend-dir'))
  const verified = JSON.parse(
    execFileSync(
      process.env.RYFRAME_PYTHON || 'python',
      [
        '-X',
        'utf8',
        path.join(backend, 'scripts/restore_reference.py'),
        args.has('--verify-existing') ? 'check-existing' : 'check-dataset',
        '--backend-dir',
        backend,
        '--plan',
        path.resolve(args.get('--plan')),
        ...(args.has('--verify-existing') ? ['--side', args.get('--side')] : []),
      ],
      { encoding: 'utf8', windowsHide: true, timeout: 60_000 },
    ),
  )
  const side = args.has('--verify-existing') ? args.get('--side') : 'source'
  if (
    verified.plan_sha256 !== hash(plan) ||
    verified.side !== side ||
    verified.scope_id !== plan[side].scope_id
  )
    throw new Error('参考计划或检查侧在ownership检查期间发生变化')
  const original = args.has('--verify-existing')
    ? await readFile(args.get('--verify-existing'))
    : null
  const result = args.has('--verify-existing')
    ? await verifyExisting(plan, backend, JSON.parse(original.toString('utf8')), side)
    : await prepareDataset(plan, backend)
  if (original) {
    if (!original.equals(await readFile(args.get('--verify-existing'))))
      throw new Error('原数据收据在验证期间发生变化')
    result.dataset_sha256 = sha256(original)
  }
  if (hash(JSON.parse(await readFile(args.get('--plan'), 'utf8'))) !== hash(plan))
    throw new Error('参考计划在验证期间发生变化')
  process.stdout.write(JSON.stringify(result) + '\n')
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  await main()
