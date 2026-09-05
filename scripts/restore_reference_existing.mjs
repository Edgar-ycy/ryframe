import { createHash } from 'node:crypto'
import { operationCatalog, Session } from './devex/request.mjs'
import { hash, httpUrl } from './devex/config.mjs'
import { requestPacer } from './restore_reference_pacing.mjs'

export function verificationSide(side = 'target') {
  if (!['source', 'target'].includes(side)) throw new Error('已有数据检查侧必须为 source 或 target')
  return side
}

export function datasetArguments(argv) {
  const args = new Map()
  const names = new Set(['--plan', '--backend-dir', '--verify-existing', '--side', '--write'])
  for (let index = 0; index < argv.length; index++) {
    const name = argv[index]
    if (!names.has(name) || args.has(name)) throw new Error('数据验收参数未知或重复')
    const value = name === '--write' ? true : argv[++index]
    if (!value || (typeof value === 'string' && value.startsWith('--')))
      throw new Error('数据验收参数缺少值')
    args.set(name, value)
  }
  if (!args.has('--plan') || !args.has('--backend-dir')) throw new Error('必须明确计划和后端目录')
  if (!args.has('--write')) throw new Error('数据准备和认证验收必须显式传入 --write')
  if (args.has('--side') && !args.has('--verify-existing'))
    throw new Error('--side 仅用于已有数据检查，数据准备固定 source')
  if (args.has('--verify-existing')) args.set('--side', verificationSide(args.get('--side')))
  return args
}

export function referenceClient(plan, catalog, identity, side = 'source') {
  verificationSide(side)
  return referenceSession(plan, catalog, identity, plan[side], side === 'source' ? 18 : 19)
}

function referenceSession(plan, catalog, identity, target, network) {
  const index = identity.tenant_id === 'system' ? 0 : Number(identity.tenant_id.slice(-2))
  if (
    identity.tenant_id !== 'system' &&
    (index < 1 ||
      index > 10 ||
      identity.tenant_id !== `${plan.source.scope_id}-${String(index).padStart(2, '0')}`)
  )
    throw new Error('参考身份不属于本次确定的十个普通租户')
  for (const value of [target.api_url, target.frontend_url]) {
    const url = httpUrl(value)
    if (!['127.0.0.1', '[::1]'].includes(url.hostname) || url.pathname !== '/')
      throw new Error('参考API与前端必须使用明确loopback根地址')
  }
  return new Session(
    {
      bindings: { api_url: target.api_url, frontend_url: target.frontend_url },
      contract: { timeout_ms: 120_000 },
    },
    catalog,
    { ...identity, client_address: `198.${network}.20.${index + 1}` },
    { beforeRequest: requestPacer(plan.dataset.request_interval_ms) },
  )
}

function validateDataset(plan, dataset) {
  if (
    dataset.format_version !== 1 ||
    dataset.plan_sha256 !== hash(plan) ||
    dataset.source_scope_id !== plan.source.scope_id ||
    !Array.isArray(dataset.tenants) ||
    dataset.tenants.length !== 11
  )
    throw new Error('旧数据收据没有绑定本次参考环境计划')
  const expected = new Set([
    'system',
    ...Array.from(
      { length: 10 },
      (_, index) => `${plan.source.scope_id}-${String(index + 1).padStart(2, '0')}`,
    ),
  ])
  for (const identity of dataset.tenants) {
    if (
      !expected.delete(identity.tenant_id) ||
      !identity.username ||
      !/^[A-Z][A-Z0-9_]+$/.test(identity.password_env) ||
      !Array.isArray(identity.posts) ||
      !identity.posts.length ||
      !Array.isArray(identity.files)
    )
      throw new Error('旧数据租户重复、归属不符或样本不完整')
    for (const post of identity.posts)
      if (!/^[1-9][0-9]*$/.test(post.id) || !post.code || !post.name)
        throw new Error('旧岗位样本无效')
    for (const file of identity.files)
      if (
        typeof file.file_path !== 'string' ||
        !file.file_path ||
        !Number.isSafeInteger(file.bytes) ||
        file.bytes <= 0 ||
        file.bytes > 4 * 1024 * 1024 ||
        !/^[a-f0-9]{64}$/.test(file.sha256)
      )
        throw new Error('旧对象样本无效')
  }
}

async function verifyFiles(apiUrl, catalog, identity, session) {
  const operation = catalog.get('get_common_file_download')
  if (operation?.method !== 'GET') throw new Error('旧对象验收必须使用只读下载契约')
  for (const file of identity.files) {
    const url = new URL(operation.path, apiUrl)
    url.searchParams.set('bucket', 'uploads')
    url.searchParams.set('path', file.file_path)
    await session.beforeRequest()
    const response = await fetch(url, {
      headers: {
        Authorization: `Bearer ${session.token}`,
        'X-Tenant-Id': identity.tenant_id,
        'X-Forwarded-For': session.identity.client_address,
      },
      signal: AbortSignal.timeout(120_000),
      redirect: 'error',
    })
    if (!response.ok) {
      await response.body?.cancel()
      throw new Error('已有对象无法通过认证业务下载')
    }
    let received = 0
    const digest = createHash('sha256')
    for await (const chunk of response.body || []) {
      received += chunk.byteLength
      if (received > file.bytes) throw new Error('已有对象响应超过登记的声明大小')
      digest.update(chunk)
    }
    if (received !== file.bytes || digest.digest('hex') !== file.sha256)
      throw new Error('已有对象下载内容不同')
  }
  return identity.files.length
}

export async function verifyExisting(plan, backend, dataset, side = 'target') {
  verificationSide(side)
  const counts = await verifyExistingAt(
    plan,
    backend,
    dataset,
    plan[side],
    side === 'source' ? 18 : 19,
  )
  return {
    format_version: 1,
    status: 'existing_data_verified',
    side,
    scope_id: plan[side].scope_id,
    plan_sha256: hash(plan),
    source_scope_id: dataset.source_scope_id,
    actions: { business: 'read_only', objects: 'read_only', session: 'login_logout' },
    restore_success: false,
    ...counts,
  }
}

async function verifyExistingAt(plan, backend, dataset, target, network) {
  validateDataset(plan, dataset)
  const catalog = await operationCatalog(backend)
  if (catalog.get('get_system_posts_by_id')?.method !== 'GET')
    throw new Error('旧岗位验收必须使用只读查询契约')
  let files = 0,
    posts = 0
  for (const identity of dataset.tenants) {
    const session = referenceSession(plan, catalog, identity, target, network)
    await session.login()
    let failure
    try {
      for (const post of identity.posts) {
        const value = await session.request({
          operation: 'get_system_posts_by_id',
          path: { id: post.id },
        })
        if (value.data.code !== post.code || value.data.name !== post.name)
          throw new Error('已有岗位内容不一致')
        posts++
      }
      files += await verifyFiles(target.api_url, catalog, identity, session)
    } catch (error) {
      failure = error
    }
    try {
      await session.request({ operation: 'post_auth_logout' })
    } catch (error) {
      failure = failure ? new AggregateError([failure, error], '已有数据验证及注销均失败') : error
    }
    if (failure) throw failure
  }
  return { tenants: dataset.tenants.length, posts, files }
}

function exactFields(value, fields) {
  if (
    !value ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    Object.keys(value).sort().join(',') !== [...fields].sort().join(',')
  )
    throw new Error('复制目标绑定字段缺失或包含未登记内容')
}

export function validateCopyTarget(plan, binding) {
  exactFields(binding, [
    'format_version',
    'kind',
    'source_plan_sha256',
    'source_scope_id',
    'target',
    'copy',
  ])
  exactFields(binding.target, ['scope_id', 'api_url', 'frontend_url'])
  exactFields(binding.copy, [
    'plan_sha256',
    'generation_sha256',
    'source_export_sha256',
    'fresh_target_sha256',
    'stage_receipt_sha256',
    'ledger_head_sha256',
  ])
  if (
    binding.format_version !== 1 ||
    binding.kind !== 'devex-copy-business-target' ||
    binding.source_plan_sha256 !== hash(plan) ||
    binding.source_scope_id !== plan.source.scope_id ||
    !/^[a-z0-9][a-z0-9_-]{2,47}$/.test(binding.target.scope_id) ||
    binding.target.scope_id === plan.source.scope_id ||
    Object.values(binding.copy).some(
      (value) => typeof value !== 'string' || !/^[a-f0-9]{64}$/.test(value),
    )
  )
    throw new Error('复制目标没有绑定原数据计划和明确的复制成功证据')
  for (const address of [binding.target.api_url, binding.target.frontend_url]) {
    const url = httpUrl(address)
    if (!['127.0.0.1', '[::1]'].includes(url.hostname) || url.pathname !== '/')
      throw new Error('复制目标API与前端必须是明确loopback根地址')
  }
}

export async function verifyCloneExisting(plan, backend, dataset, binding) {
  // Python 调用方先后核验真实 stage、账本、ownership 与 API 内核身份；此处只消费其明确绑定。
  validateCopyTarget(plan, binding)
  validateDataset(plan, dataset)
  const original = { plan: hash(plan), dataset: hash(dataset), target: hash(binding) }
  const snapshot = structuredClone({ plan, dataset, target: binding.target })
  const counts = await verifyExistingAt(
    snapshot.plan,
    backend,
    snapshot.dataset,
    snapshot.target,
    19,
  )
  if (
    hash(plan) !== original.plan ||
    hash(dataset) !== original.dataset ||
    hash(binding) !== original.target
  )
    throw new Error('原数据来源或复制目标在业务验证期间发生变化')
  return {
    format_version: 1,
    status: 'copy_existing_data_verified',
    side: 'copy_target',
    scope_id: binding.target.scope_id,
    plan_sha256: original.plan,
    source_scope_id: dataset.source_scope_id,
    target_binding_sha256: original.target,
    copy: structuredClone(binding.copy),
    actions: { business: 'read_only', objects: 'read_only', session: 'login_logout' },
    clone_verified: false,
    restore_success: false,
    ...counts,
  }
}
