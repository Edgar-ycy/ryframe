import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { createIdentityPlan, digest } from './identity-plan.mjs'

export async function identityFixture(t) {
  const root = path.resolve('.local-tests')
  await mkdir(root, { recursive: true })
  const directory = await mkdtemp(path.join(root, 'identity-tool-'))
  t.after(async () => { if (path.dirname(directory) !== root) throw new Error('清理越界'); await rm(directory, { recursive: true }) })
  const document = path.join(directory, 'environment.md'), runtime = path.join(directory, 'runtime')
  await mkdir(runtime)
  await writeFile(document, '独立性能测试环境')
  await writeFile(path.join(runtime, 'runtime.json'), '{}')
  await writeFile(path.join(runtime, 'api.json'), '{}')
  const connection = (database) => ({ host: '127.0.0.1', port: 3306, database, username: 'fixture',
    password_env: 'APP_IDENTITY_DB_PASSWORD', tls_mode: 'required' })
  const admin = (tenant_id, index) => ({ tenant_id, username: 'admin', password_env: 'RYFRAME_IDENTITY_ADMIN_PASSWORD',
    client_address: `198.18.30.${index + 1}` })
  const environment = { format_version: 1, plan_id: 'fixture', scope_id: 'identity-fixture', backend_dir: process.cwd(),
    frontend_dir: path.resolve('../ryframe-vue3'), api_url: 'http://127.0.0.1:18999/', frontend_url: 'http://127.0.0.1:4199/',
    environment_document: { path: document, sha256: digest('独立性能测试环境') },
    runtime: { directory: runtime, receipt_sha256: digest('{}'), api_process_sha256: digest('{}') },
    database: { python: path.resolve('python-fixture'), mysql_client: path.resolve('mysql-fixture'),
      server_uuid: '11111111-1111-1111-1111-111111111111', control: connection('identity_control'),
      targets: [{ key: 'shared', mode: 'shared', connection: connection('identity_shared') }] },
    system_admin: admin('system', 0),
    tenants: Array.from({ length: 10 }, (_, index) => ({ slot: `tenant-${String(index + 1).padStart(2, '0')}`,
      tenant_id: `fixture-${index + 1}`, target_key: 'shared', admin: admin(`fixture-${index + 1}`, index + 1) })),
    passwords: { system_env: 'RYFRAME_IDENTITY_SYSTEM_PASSWORD', tenant_env: 'RYFRAME_IDENTITY_TENANT_PASSWORD' },
    quota: { system_max_users: 1000, tenant_max_users: 1000, import_headroom_per_tenant: 100 },
    pacing: { contract: { authority_sha256: 'a'.repeat(64), request_interval_ms: 1000,
      operation_intervals_ms: { post_auth_login: 12250 }, sample_prepare_wait_ms: 60250 },
    bindings: { python: path.resolve('python-fixture'), login_budget_state: path.join(directory, 'login-budget.json') } } }
  for (const key of ['RYFRAME_IDENTITY_SYSTEM_PASSWORD', 'RYFRAME_IDENTITY_TENANT_PASSWORD']) {
    const previous = process.env[key]
    process.env[key] = 'Secret!Fixture123'
    t.after(() => { if (previous === undefined) delete process.env[key]; else process.env[key] = previous })
  }
  return { directory, environment, plan: await createIdentityPlan(environment) }
}

export function identityBackend(plan, options = {}) {
  const roles = new Map(), users = new Map(), resets = new Map(), events = []
  let sequence = 10, closed = 0, interrupted = false
  const groupFor = (tenant) => plan.groups.find((group) => group.tenant_id === tenant)
  const userKey = (identity) => `${identity.tenant_id}/${identity.username}`
  const permissionTree = (group) => group.permissions.map((code, index) => ({ id: String(1000 + index), code, status: '1', children: [] }))
  class Session {
    constructor(_config, _catalog, identity, controls) {
      this.identity = identity; this.beforeRequest = controls.beforeRequest
    }
    async login() {
      events.push('login')
      if (this.identity.username !== 'admin' && users.get(userKey(this.identity))?.status !== '1') throw new Error('未激活')
    }
    async request(step) {
      const operation = step.operation, group = groupFor(this.identity.tenant_id)
      events.push(operation)
      if (options.interruptAfterSystem && !interrupted && operation === 'get_system_perms_tree' &&
          group.slot === 'tenant-01' && users.size === 100) {
        interrupted = true
        throw new Error('fixture clean interruption')
      }
      if (operation === 'get_auth_csrf' || operation === 'post_auth_logout') return { data: {} }
      if (operation === 'get_auth_context') {
        const admin = this.identity.username === 'admin', user = users.get(userKey(this.identity))
        return { data: { user: { id: admin ? '1' : user.id, tenant_id: group.tenant_id, username: this.identity.username },
          is_super_admin: admin, roles: admin ? ['admin'] : [group.role_code], permissions: admin ? [] : group.permissions } }
      }
      if (operation === 'get_system_perms_tree') return { data: permissionTree(group) }
      if (operation === 'get_system_roles') return { data: { total: roles.has(group.slot) ? 1 : 0 } }
      if (operation === 'get_system_users') return { data: { total: [...users.values()].filter((user) => user.tenant_id === group.tenant_id).length } }
      if (operation === 'post_system_roles') {
        const role = { id: String(++sequence), code: step.body.code, is_super: 0, status: '1', data_scope: '1', permissions: [] }
        roles.set(group.slot, role); return { data: role }
      }
      if (operation === 'put_system_roles_by_id_permissions') {
        roles.get(group.slot).permissions = step.body.perm_ids; return { data: null }
      }
      if (operation === 'get_system_roles_by_id_permissions') return { data: roles.get(group.slot).permissions }
      if (operation === 'get_system_roles_by_id') return { data: roles.get(group.slot) }
      if (operation === 'post_system_users') {
        const user = { id: String(++sequence), tenant_id: group.tenant_id, username: step.body.username,
          status: 'pending_activation', roles: [roles.get(group.slot)] }
        users.set(userKey(user), user)
        if (options.unknownUserCreate) throw new Error('响应在提交后丢失，private secret')
        return { data: user }
      }
      if (operation === 'get_system_users_by_id') return { data: [...users.values()].find((user) => user.id === step.path.id) }
      if (operation === 'post_system_users_by_id_password_reset_requests') {
        const id = String(++sequence); resets.set(id, step.path.id)
        return { data: { request_id: id, expires_at: new Date(Date.now() + 60000).toISOString(),
          reset_url: `/reset-password#tenant_id=${group.tenant_id}&request_id=${id}&token=secret-test-reset-token` } }
      }
      if (operation === 'post_auth_password_reset_complete') {
        if (step.body.new_password !== 'Secret!Fixture123' || step.body.token !== 'secret-test-reset-token') throw new Error('激活载荷错误')
        const user = [...users.values()].find((item) => item.id === resets.get(step.body.request_id))
        user.status = '1'; return { data: null }
      }
      throw new Error('未知操作')
    }
  }
  const dependencies = {
    Session,
    operationCatalog: async () => new Map(),
    createPacing: async () => ({ createPreparationControls: async () => ({ beforeRequest: async () => {} }), close: async () => { closed++ } }),
    python: async (_executable, request) => {
      events.push(request.operation)
      if (request.operation === 'inspect') return { scope_id: plan.environment.scope_id,
        server_uuid: plan.environment.database.server_uuid, api_process: { pid: 123 } }
      return { template_sha256: request.sha256, department_path: options.differentTemplate && request.path.includes('tenant-10') ? '其他部门' : '总公司',
        department_sha256: digest('总公司') }
    },
    downloadTemplate: async (_context, _session, file) => { await writeFile(file, 'fixture-template'); return digest('fixture-template') },
  }
  return { dependencies, events, roles, users, closed: () => closed }
}
