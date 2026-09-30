import path from 'node:path'
import { lstat, readFile, realpath } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { hash, httpUrl } from './config.mjs'

export const identityPermissions = Object.freeze({
  system: ['monitor:schedule:add', 'monitor:schedule:run', 'system:message:publish', 'system:post:export'],
  tenant: ['system:post:add', 'system:post:edit', 'system:post:export', 'system:post:list',
    'system:post:remove', 'system:user-import:add', 'system:user-import:list'],
})
export const identifier = (value) => typeof value === 'string' && /^[a-z0-9][a-z0-9-]{2,31}$/.test(value)
export const secretName = (value) => typeof value === 'string' && /^[A-Z][A-Z0-9_]*$/.test(value)
export const digest = (value) => createHash('sha256').update(value).digest('hex')
export function exact(value, fields, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      Object.keys(value).sort().join() !== fields.slice().sort().join()) throw new Error(`${label}字段不完整`)
}

export async function localPath(backend, filename, exists = true) {
  const root = path.resolve(backend, '.local-tests')
  if ((await lstat(root)).isSymbolicLink()) throw new Error('本地测试根目录不能是链接')
  if (typeof filename !== 'string' || !path.isAbsolute(filename)) throw new Error('必须使用明确绝对路径')
  const target = path.resolve(filename), relative = path.relative(root, target)
  if (!relative || relative.startsWith('..') || path.isAbsolute(relative)) throw new Error('路径必须位于当前后端.local-tests内')
  let cursor = root
  for (const part of relative.split(path.sep)) {
    cursor = path.join(cursor, part)
    try { if ((await lstat(cursor)).isSymbolicLink()) throw new Error('路径不得经过链接') }
    catch (error) { if (error.code !== 'ENOENT' || exists) throw error }
  }
  if (exists && path.relative(root, await realpath(target)) !== relative) throw new Error('路径解析越界')
  return target
}

function credential(value, tenant, label) {
  exact(value, ['tenant_id', 'username', 'password_env', 'client_address'], label)
  if (value.tenant_id !== tenant || typeof value.username !== 'string' || !value.username.trim() ||
      !secretName(value.password_env) || !/^198\.(18|19)\.\d{1,3}\.\d{1,3}$/.test(value.client_address) ||
      value.client_address.split('.').some((part) => Number(part) > 255 || String(Number(part)) !== part))
    throw new Error(`${label}身份或固定地址无效`)
}

function connection(value) {
  exact(value, ['host', 'port', 'database', 'username', 'password_env', 'tls_mode'], '数据库绑定')
  if (!['127.0.0.1', 'localhost'].includes(value.host) || !Number.isInteger(value.port) || value.port < 1 || value.port > 65535 ||
      !/^[a-zA-Z0-9_]{1,64}$/.test(value.database) || typeof value.username !== 'string' || !value.username ||
      !secretName(value.password_env) || !['required', 'disabled'].includes(value.tls_mode))
    throw new Error('数据库必须是明确的本机隔离目标')
}

export function validateEnvironment(value) {
  exact(value, ['format_version', 'plan_id', 'scope_id', 'backend_dir', 'frontend_dir', 'api_url', 'frontend_url',
    'environment_document', 'runtime', 'database', 'system_admin', 'tenants', 'passwords', 'quota', 'pacing'], '环境清单')
  if (value.format_version !== 1 || !identifier(value.plan_id) || !/^[a-z0-9][a-z0-9-]{2,63}$/.test(value.scope_id))
    throw new Error('准备计划版本或scope无效')
  for (const key of ['backend_dir', 'frontend_dir']) if (!path.isAbsolute(value[key])) throw new Error('源码目录必须明确')
  for (const key of ['api_url', 'frontend_url']) {
    const url = httpUrl(value[key])
    if (!['127.0.0.1', '[::1]'].includes(url.hostname) || url.pathname !== '/') throw new Error('准备入口必须为loopback根地址')
  }
  exact(value.environment_document, ['path', 'sha256'], '环境说明')
  exact(value.runtime, ['directory', 'receipt_sha256', 'api_process_sha256'], '运行来源')
  for (const item of [value.environment_document.sha256, value.runtime.receipt_sha256, value.runtime.api_process_sha256])
    if (!/^[a-f0-9]{64}$/.test(item)) throw new Error('准备来源必须绑定真实SHA')
  exact(value.database, ['python', 'mysql_client', 'server_uuid', 'control', 'targets'], '数据库来源')
  if (!path.isAbsolute(value.database.python) || !path.isAbsolute(value.database.mysql_client) ||
      !/^[a-f0-9-]{36}$/.test(value.database.server_uuid) || !Array.isArray(value.database.targets)) throw new Error('数据库来源无效')
  connection(value.database.control)
  const targets = new Set()
  for (const target of value.database.targets) {
    exact(target, ['key', 'mode', 'connection'], '租户目标')
    if (!identifier(target.key) || targets.has(target.key) || !['shared', 'dedicated'].includes(target.mode)) throw new Error('租户目标重复或无效')
    connection(target.connection); targets.add(target.key)
    if (!target.connection.password_env.startsWith('APP_')) throw new Error('目标密码必须由运行收据的APP环境摘要绑定')
  }
  credential(value.system_admin, 'system', '系统准备管理员')
  if (!Array.isArray(value.tenants) || value.tenants.length !== 10) throw new Error('必须登记十个普通租户')
  const tenants = new Set(), addresses = new Set([value.system_admin.client_address])
  value.tenants.forEach((tenant, index) => {
    exact(tenant, ['slot', 'tenant_id', 'target_key', 'admin'], '普通租户')
    if (tenant.slot !== `tenant-${String(index + 1).padStart(2, '0')}` || tenant.tenant_id === 'system' ||
        !/^[a-zA-Z0-9_-]{1,64}$/.test(tenant.tenant_id) || tenants.has(tenant.tenant_id) || !targets.has(tenant.target_key))
      throw new Error('实际十租户映射无效')
    credential(tenant.admin, tenant.tenant_id, '租户准备管理员')
    if (addresses.has(tenant.admin.client_address)) throw new Error('准备管理员客户地址必须独立')
    addresses.add(tenant.admin.client_address); tenants.add(tenant.tenant_id)
  })
  const physical = new Set()
  for (const target of value.database.targets) {
    const count = value.tenants.filter((tenant) => tenant.target_key === target.key).length
    const key = `${target.connection.port}/${target.connection.database.toLowerCase()}`
    if (!count || (target.mode === 'dedicated' && count !== 1) || physical.has(key)) throw new Error('目标必须被明确使用且不能重复或共享独立库')
    physical.add(key)
  }
  exact(value.passwords, ['system_env', 'tenant_env'], '普通用户密码')
  if (!Object.values(value.passwords).every(secretName)) throw new Error('只允许密码环境变量名称')
  exact(value.quota, ['system_max_users', 'tenant_max_users', 'import_headroom_per_tenant'], '预登记配额')
  if (!Object.values(value.quota).every((item) => Number.isSafeInteger(item) && item > 0) ||
      value.quota.system_max_users < 101 || value.quota.tenant_max_users < 11 + value.quota.import_headroom_per_tenant)
    throw new Error('配额必须预先覆盖管理员、固定普通身份和导入余量')
  exact(value.pacing, ['contract', 'bindings'], '请求节奏')
  exact(value.pacing.contract, ['authority_sha256', 'request_interval_ms', 'operation_intervals_ms', 'sample_prepare_wait_ms'], '节奏契约')
  exact(value.pacing.bindings, ['python', 'login_budget_state'], '节奏绑定')
  const pace = value.pacing.contract
  if (!/^[a-f0-9]{64}$/.test(pace.authority_sha256) || !Number.isSafeInteger(pace.request_interval_ms) || pace.request_interval_ms < 1 ||
      !Number.isSafeInteger(pace.sample_prepare_wait_ms) || pace.sample_prepare_wait_ms < 0 ||
      !pace.operation_intervals_ms || typeof pace.operation_intervals_ms !== 'object' || Array.isArray(pace.operation_intervals_ms) ||
      Object.entries(pace.operation_intervals_ms).some(([operation, interval]) => !/^[a-z][a-z0-9_]*$/.test(operation) ||
        !Number.isSafeInteger(interval) || interval < 1) || !path.isAbsolute(value.pacing.bindings.python) ||
      !path.isAbsolute(value.pacing.bindings.login_budget_state)) throw new Error('节奏参数或绑定无效')
  return value
}

export async function createIdentityPlan(environment) {
  validateEnvironment(environment)
  const backend = environment.backend_dir
  const document = await localPath(backend, environment.environment_document.path)
  const bytes = await readFile(document)
  if (!bytes.toString('utf8').trim() || digest(bytes) !== environment.environment_document.sha256) throw new Error('环境说明SHA不匹配')
  const directory = await localPath(backend, environment.runtime.directory)
  await localPath(backend, environment.pacing.bindings.login_budget_state, false)
  for (const [file, expected] of [['runtime.json', environment.runtime.receipt_sha256], ['api.json', environment.runtime.api_process_sha256]]) {
    if (digest(await readFile(await localPath(backend, path.join(directory, file)))) !== expected) throw new Error('运行收据SHA不匹配')
  }
  const groups = [{ slot: 'system', tenant_id: 'system', admin: environment.system_admin, count: 100, kind: 'system' },
    ...environment.tenants.map((tenant) => ({ slot: tenant.slot, tenant_id: tenant.tenant_id, admin: tenant.admin, count: 10, kind: 'tenant' }))]
  for (const [groupIndex, group] of groups.entries()) {
    group.role_code = `dv-${environment.plan_id}-${group.kind}`
    group.permissions = [...identityPermissions[group.kind]].sort()
    group.users = Array.from({ length: group.count }, (_, index) => ({
      user_slot: `${group.slot}-user-${String(index + 1).padStart(3, '0')}`,
      username: `dv-${environment.plan_id}-${String(index + 1).padStart(3, '0')}`,
      password_env: environment.passwords[`${group.kind}_env`], client_address: `198.18.${40 + groupIndex}.${index + 1}`,
    }))
    if (group.role_code.length > 50 || group.users.some((user) => user.username.length > 50)) throw new Error('准备名称超出当前契约')
  }
  const administratorAddresses = new Set(groups.map((group) => group.admin.client_address))
  if (groups.some((group) => group.users.some((user) => administratorAddresses.has(user.client_address))))
    throw new Error('普通用户固定客户地址不得与准备管理员重叠')
  const plan = { format_version: 1, kind: 'devex-identities', environment: structuredClone(environment), groups }
  return { ...plan, plan_sha256: hash(plan) }
}

export function validateIdentityPlan(plan) {
  exact(plan, ['format_version', 'kind', 'environment', 'groups', 'plan_sha256'], '身份计划')
  validateEnvironment(plan.environment)
  const { plan_sha256, ...payload } = plan
  if (plan.format_version !== 1 || plan.kind !== 'devex-identities' || hash(payload) !== plan_sha256)
    throw new Error('身份计划摘要或版本无效')
  return plan
}
