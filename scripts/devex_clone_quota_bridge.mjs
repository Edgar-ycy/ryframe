import path from 'node:path'
import readline from 'node:readline'
import { createHash } from 'node:crypto'
import { lstat, readFile } from 'node:fs/promises'
import { isDeepStrictEqual } from 'node:util'
import { fileURLToPath, pathToFileURL } from 'node:url'

const fields = ['name', 'domain', 'expire_at', 'max_users', 'max_roles', 'max_storage_mb', 'max_requests_per_min']
const permissions = ['tenant:list', 'tenant:edit', 'tenant:usage:list']
function exact(value, keys) {
  if (!value || typeof value !== 'object' || Array.isArray(value)
      || !isDeepStrictEqual(Object.keys(value).sort(), [...keys].sort())) throw new Error('配额请求字段不完整')
}

export function quotaProducerArguments(argv) {
  const names = ['--run-dir', '--attempt', '--quota-mode', '--identity-plan', '--identity-plan-sha256']
  const options = new Map()
  for (let index = 0; index < argv.length; index++) {
    const key = argv[index], value = argv[++index]
    if (!names.includes(key) || options.has(key) || !value || value.startsWith('--')) throw new Error('配额生产者参数无效')
    options.set(key, value)
  }
  if (options.size !== names.length || !['--run-dir', '--identity-plan'].every((key) => path.isAbsolute(options.get(key)))
      || !/^[1-9][0-9]*$/.test(options.get('--attempt')) || !Number.isSafeInteger(Number(options.get('--attempt')))
      || !['plan', 'apply', 'reconcile'].includes(options.get('--quota-mode'))
      || !/^[a-f0-9]{64}$/.test(options.get('--identity-plan-sha256'))) throw new Error('配额生产者必须绑定当前运行和身份计划')
  return { runDirectory: options.get('--run-dir'), attempt: Number(options.get('--attempt')),
    mode: options.get('--quota-mode'), plan: options.get('--identity-plan'), planSha256: options.get('--identity-plan-sha256') }
}

export function validateQuotaGrant(args, request, environment) {
  exact(request, ['id', 'operation', 'backend', 'frontend', 'config', 'identity', 'artifacts', 'run_dir', 'attempt', 'tenant_ids'])
  if (request.operation !== 'initialize' || request.run_dir !== args.runDirectory || request.attempt !== args.attempt
      || path.resolve(request.backend) !== process.cwd() || request.backend !== environment.backend_dir
      || request.frontend !== environment.frontend_dir
      || request.artifacts !== path.join(args.runDirectory, 'seed-runtime', `attempt-${String(args.attempt).padStart(4, '0')}`, 'pacing'))
    throw new Error('会话授权不属于当前 run、attempt 或固定源码')
  const tenants = ['system', ...environment.tenants.map((item) => item.tenant_id)]
  if (tenants.length !== 11 || new Set(tenants).size !== 11 || !isDeepStrictEqual(request.tenant_ids, tenants))
    throw new Error('配额入口只允许身份计划明确登记的十一个租户')
  const expected = { contract: { timeout_ms: 120000, pacing: environment.pacing.contract },
    bindings: { scope_id: environment.scope_id, api_url: environment.api_url,
      frontend_url: environment.frontend_url, pacing: environment.pacing.bindings } }
  const config = structuredClone(request.config)
  for (const key of ['api_url', 'frontend_url']) {
    if (new URL(config.bindings[key]).href !== new URL(expected.bindings[key]).href) throw new Error('会话端点不同')
    config.bindings[key] = expected.bindings[key]
  }
  if (!isDeepStrictEqual(config, expected) || request.identity.tenant_id !== 'system'
      || typeof request.identity.subject_id !== 'string' || !/^[1-9][0-9]*$/.test(request.identity.subject_id)
      || Object.entries(environment.system_admin).some(([key, value]) => request.identity[key] !== value))
    throw new Error('配额入口身份、端点或节奏与登记不符')
}

export function quotaAuthorization(response, identity) {
  const auth = response?.data
  if (!auth || auth.user?.id !== identity.subject_id || auth.user?.tenant_id !== 'system'
      || auth.user?.username !== identity.username || !Array.isArray(auth.permissions)
      || !(auth.is_super_admin === true || auth.permissions.includes('*') || permissions.every((key) => auth.permissions.includes(key))))
    throw new Error('配额会话的系统管理员或权威权限不符')
  return { subject_id: auth.user.id, tenant_id: auth.user.tenant_id, username: auth.user.username,
    is_super_admin: auth.is_super_admin, permissions: auth.permissions }
}

export class QuotaSession {
  constructor(mode, environment, identity, session) {
    this.mode = mode; this.identity = identity; this.session = session
    this.desired = new Map([['system', environment.quota.system_max_users],
      ...environment.tenants.map((item) => [item.tenant_id, environment.quota.tenant_max_users])])
    this.before = new Map(); this.written = new Set(); this.failed = false
  }

  async operation(request) {
    if (this.failed) throw new Error('会话已经失败，禁止继续请求或重放')
    try { return await this.execute(request) }
    catch (error) { this.failed = true; throw error }
  }

  async execute(request) {
    const update = request.operation === 'update'
    if (!['get', 'usage', 'update'].includes(request.operation)) throw new Error('未知配额动作')
    exact(request, update ? ['id', 'operation', 'tenant_id', 'body'] : ['id', 'operation', 'tenant_id'])
    if (!this.desired.has(request.tenant_id)) throw new Error('租户未登记')
    const target = { tenant_id: request.tenant_id }
    if (request.operation === 'get') {
      const result = await this.session.request({ operation: 'get_platform_tenants_by_tenant_id', path: target })
      if (result?.data?.tenant_id !== request.tenant_id) throw new Error('租户读取响应不匹配')
      if (fields.filter(key => key.startsWith('max_')).some(key => !Number.isSafeInteger(result.data[key]) || result.data[key] < 0))
        throw new Error('配额前像不能包含无法精确保留的整数')
      this.before.set(request.tenant_id, structuredClone(result.data))
      return result
    }
    if (request.operation === 'usage')
      return this.session.request({ operation: 'get_platform_tenants_by_tenant_id_usage', path: target })
    exact(request.body, fields)
    const before = this.before.get(request.tenant_id), desired = this.desired.get(request.tenant_id)
    if (this.mode !== 'apply' || this.written.has(request.tenant_id) || !before || before.status !== 'enabled'
        || !Number.isSafeInteger(desired) || desired <= 0 || !Number.isSafeInteger(before.max_users)
        || before.max_users <= 0 || desired <= before.max_users || request.body.max_users !== desired
        || fields.some((key) => key !== 'max_users' && !isDeepStrictEqual(request.body[key], before[key])))
      throw new Error('只允许一次增加登记配额，且必须保持完整前像的其余字段')
    // 登记发送状态后仅发送一次；任何响应、刷新或鉴权失败都由外层保留未知意图。
    this.written.add(request.tenant_id)
    const response = await this.session.request({ operation: 'put_platform_tenants_by_tenant_id', path: target, body: request.body })
    const previousToken = this.session.token
    await this.session.request({ operation: 'post_auth_refresh' })
    if (typeof this.session.token !== 'string' || !this.session.token || this.session.token === previousToken)
      throw new Error('配额更新后的 access token 尚未轮换')
    const authorization = quotaAuthorization(await this.session.request({ operation: 'get_auth_context' }), this.identity)
    return { response, authorization }
  }
}

async function dependencies(backend) {
  const { Session, operationCatalog } = await import(pathToFileURL(path.join(backend, 'scripts/devex/request.mjs')))
  const { createPacing } = await import(pathToFileURL(path.join(backend, 'scripts/devex/pacing.mjs')))
  return { Session, operationCatalog, createPacing }
}

async function identityPlan(args, backend) {
  const { localPath, validateIdentityPlan } = await import(pathToFileURL(path.join(backend, 'scripts/devex/identity-plan.mjs')))
  const filename = await localPath(backend, args.plan)
  if (!(await lstat(filename)).isFile()) throw new Error('身份计划必须为普通文件')
  const content = await readFile(filename)
  if (createHash('sha256').update(content).digest('hex') !== args.planSha256) throw new Error('登记身份计划摘要发生变化')
  return validateIdentityPlan(JSON.parse(content)).environment
}

export async function runQuotaBridge(argv, input = process.stdin, output = process.stdout, load = dependencies, readPlan = identityPlan) {
  const args = quotaProducerArguments(argv)
  const lines = readline.createInterface({ input, crlfDelay: Infinity })
  let controller, session, pacing, lastId = 0, closed = false
  async function close() {
    try {
      if (session && !closed) {
        closed = true
        let timer
        try { await Promise.race([session.request({ operation: 'post_auth_logout' }),
          new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('注销超时')), 10000) })]) }
        finally { clearTimeout(timer) }
      }
    } finally { await pacing?.close(); pacing = undefined }
  }
  try {
    for await (const line of lines) {
      let id
      try {
        if (Buffer.byteLength(line) > 1024 * 1024) throw new Error('输入过大')
        const request = JSON.parse(line); id = request.id
        if (!Number.isSafeInteger(id) || id !== lastId + 1) throw new Error('响应序号不连续')
        lastId = id
        let data
        if (!controller) {
          // 收到正确 run/attempt 的 stdin 授权以前不导入 API、不读取登录秘密或创建会话。
          if (request.operation !== 'initialize' || request.run_dir !== args.runDirectory || request.attempt !== args.attempt
              || request.backend !== process.cwd()) throw new Error('首个授权不属于当前生产者')
          const environment = await readPlan(args, request.backend)
          validateQuotaGrant(args, request, environment)
          const modules = await load(request.backend)
          pacing = await modules.createPacing(request.config, { backend: request.backend,
            runner_frontend: request.frontend, artifacts: request.artifacts })
          session = new modules.Session(request.config, await modules.operationCatalog(request.backend), request.identity,
            await pacing.createPreparationControls(request.identity))
          await session.login()
          data = quotaAuthorization(await session.request({ operation: 'get_auth_context' }), request.identity)
          controller = new QuotaSession(args.mode, environment, request.identity, session)
        } else if (request.operation === 'close') {
          exact(request, ['id', 'operation']); await close(); data = { closed: true }
        } else data = await controller.operation(request)
        output.write(JSON.stringify({ id, ok: true, data }) + '\n')
        if (request.operation === 'close') break
      } catch (error) {
        output.write(JSON.stringify({ id, ok: false, error_type: error?.name ?? 'Error',
          status: Number.isInteger(error?.status) ? error.status : null }) + '\n')
        break
      }
    }
  } finally { lines.close(); await close() }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { await runQuotaBridge(process.argv.slice(2)) }
  catch { process.stderr.write('quota_bridge_failed：配额阶段未完成，请核对原阶段收据与意图。\n'); process.exitCode = 1 }
}
