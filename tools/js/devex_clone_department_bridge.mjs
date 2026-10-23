import path from 'node:path'
import readline from 'node:readline'
import { createHash } from 'node:crypto'
import { lstat, readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { DepartmentSession, departmentAuthorization, departmentTargets,
  departmentTemplateAuthorization, validateDepartmentGrant } from './department-stage.mjs'

const names = ['--run-dir', '--attempt', '--department-mode', '--identity-plan', '--identity-plan-sha256']

export function departmentProducerArguments(argv) {
  const options = new Map()
  for (let index = 0; index < argv.length; index++) {
    const key = argv[index], value = argv[++index]
    if (!names.includes(key) || options.has(key) || !value || value.startsWith('--')) throw new Error('部门生产者参数无效')
    options.set(key, value)
  }
  if (options.size !== names.length || !['--run-dir', '--identity-plan'].every((key) => path.isAbsolute(options.get(key))) ||
      !/^[1-9][0-9]*$/.test(options.get('--attempt')) || !Number.isSafeInteger(Number(options.get('--attempt'))) ||
      !['plan', 'apply', 'reconcile', 'verify'].includes(options.get('--department-mode')) ||
      !/^[a-f0-9]{64}$/.test(options.get('--identity-plan-sha256')))
    throw new Error('部门生产者必须绑定当前run、attempt、模式及身份计划')
  return { runDirectory: options.get('--run-dir'), attempt: Number(options.get('--attempt')),
    mode: options.get('--department-mode'), plan: options.get('--identity-plan'), planSha256: options.get('--identity-plan-sha256') }
}

async function dependencies(backend) {
  const { Session, operationCatalog } = await import(pathToFileURL(path.join(backend, 'tools/js/request.mjs')))
  const { createPacing } = await import(pathToFileURL(path.join(backend, 'tools/js/pacing.mjs')))
  class FileSession extends Session {
    async login() {
      const password = this.identity.password
      if (typeof password !== 'string' || !password) throw new Error('缺少文件配置的身份密码')
      await this.request({ operation: 'get_auth_csrf' })
      const response = await this.request({ operation: 'post_auth_login', body: { username: this.identity.username, password } })
      if (typeof response?.data?.access_token !== 'string' || !response.data.access_token) throw new Error('登录缺少当前会话令牌')
      this.token = response.data.access_token
    }
  }
  return { Session: FileSession, operationCatalog, createPacing }
}

async function identityPlan(args, backend) {
  const filename = path.resolve(args.plan)
  const local = path.join(backend, '.local-tests')
  if (!filename.startsWith(local + path.sep) || filename !== args.plan) throw new Error('身份计划必须位于当前隔离目录')
  for (let current = filename; current !== backend; current = path.dirname(current)) {
    const info = await lstat(current)
    if (info.isSymbolicLink() || (current === filename && (!info.isFile() || info.size > 16 * 1024 * 1024))) throw new Error('身份计划必须为无链接的有界普通文件')
  }
  const content = await readFile(filename)
  if (createHash('sha256').update(content).digest('hex') !== args.planSha256) throw new Error('登记身份计划摘要发生变化')
  const plan = JSON.parse(content)
  const { hash } = await import(pathToFileURL(path.join(backend, 'tools/js/config.mjs')))
  const { plan_sha256, ...payload } = plan
  if (plan.format_version !== 1 || plan.kind !== 'devex-identities' || hash(payload) !== plan_sha256) throw new Error('Rust 身份计划摘要无效')
  departmentTargets(plan)
  const configuration = path.resolve(plan.configuration.path)
  if (!configuration.startsWith(local + path.sep) || configuration !== plan.configuration.path) throw new Error('私有配置必须位于当前隔离目录')
  for (let current = configuration; current !== backend; current = path.dirname(current)) {
    if ((await lstat(current)).isSymbolicLink()) throw new Error('私有配置不能经过链接')
  }
  const metadata = await lstat(configuration)
  if (!metadata.isFile() || metadata.size > 16 * 1024 * 1024 || metadata.size !== plan.configuration.bytes) throw new Error('私有配置大小不同')
  const bytes = await readFile(configuration)
  if (createHash('sha256').update(bytes).digest('hex') !== plan.configuration.sha256) throw new Error('私有配置原始摘要变化')
  const privateConfiguration = JSON.parse(bytes)
  const publicConfiguration = structuredClone(privateConfiguration)
  delete publicConfiguration.passwords
  delete publicConfiguration.system_admin.password
  delete publicConfiguration.database.control.password
  for (const item of publicConfiguration.database.targets) delete item.connection.password
  for (const item of publicConfiguration.tenants) delete item.admin.password
  if (hash(publicConfiguration) !== hash(plan.environment)) throw new Error('私有配置绑定与公开计划不同')
  plan.environment = privateConfiguration
  for (const group of plan.groups) {
    group.admin = group.kind === 'system' ? privateConfiguration.system_admin : privateConfiguration.tenants.find((tenant) => tenant.tenant_id === group.tenant_id).admin
  }
  return plan
}

export async function runDepartmentBridge(argv, input = process.stdin, output = process.stdout,
    load = dependencies, readPlan = identityPlan, closeTimeoutMs = 10000) {
  const args = departmentProducerArguments(argv)
  if (!Number.isSafeInteger(closeTimeoutMs) || closeTimeoutMs <= 0) throw new Error('部门关闭超时无效')
  const lines = readline.createInterface({ input, crlfDelay: Infinity })
  let controller, pacing, catalog, plan, config, SessionType, lastId = 0, closed = false
  const sessions = new Map()
  async function boundedClose(operation, message) {
    let timer
    try {
      return await Promise.race([Promise.resolve().then(operation),
        new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(message)), closeTimeoutMs) })])
    } finally { clearTimeout(timer) }
  }
  async function logout(session) {
    if (session.token) await boundedClose(
      () => session.request({ operation: 'post_auth_logout' }), '注销超时')
  }
  async function close() {
    if (closed) return
    closed = true
    const failures = []
    try {
      for (const value of sessions.values()) {
        if (!value.session.token) continue
        try { await logout(value.session) }
        catch (error) { failures.push(error) }
      }
    } finally {
      try { await boundedClose(() => pacing?.close(), '节流器关闭超时') }
      catch (error) { failures.push(error) }
      pacing = undefined
    }
    if (failures.length) throw new AggregateError(failures, '部门会话清理失败')
  }
  async function sessionFor(target, purpose, mode) {
    if (!['owner', 'template'].includes(purpose)) throw new Error('部门会话用途无效')
    const key = `${purpose}:${target.tenant_id}`
    if (sessions.has(key)) return sessions.get(key)
    const identity = purpose === 'owner' ? target.admin : target.template_identity
    const session = new SessionType(config, catalog, identity,
      await pacing.createPreparationControls(identity))
    try {
      await session.login()
      const context = await session.request({ operation: 'get_auth_context' })
      const authorization = purpose === 'owner'
        ? departmentAuthorization(context, target.admin, mode)
        : departmentTemplateAuthorization(context, target.template_identity)
      const value = { session, authorization }
      sessions.set(key, value)
      return value
    } catch (error) {
      if (session.token) {
        try { await logout(session) }
        catch (cleanup) { throw new AggregateError([error, cleanup], '部门会话初始化及清理失败') }
      }
      throw error
    }
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
          if (request.operation !== 'initialize' || request.run_dir !== args.runDirectory || request.attempt !== args.attempt ||
              request.backend !== process.cwd()) throw new Error('首个授权不属于当前部门生产者')
          plan = await readPlan(args, request.backend)
          data = validateDepartmentGrant(args, request, plan)
          const modules = await load(request.backend)
          pacing = await modules.createPacing(request.config, { backend: request.backend,
            runner_frontend: request.frontend, artifacts: request.artifacts })
          catalog = await modules.operationCatalog(request.backend)
          SessionType = modules.Session
          config = request.config
          controller = new DepartmentSession(args.mode, plan, request.template_identities, sessionFor)
        } else if (request.operation === 'close') {
          if (Object.keys(request).sort().join() !== 'id,operation') throw new Error('关闭请求字段不完整')
          await close(); data = { closed: true }
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
  try { await runDepartmentBridge(process.argv.slice(2)) }
  catch { process.stderr.write('department_bridge_failed：部门阶段未完成，请核对原阶段收据与意图。\n'); process.exitCode = 1 }
}
