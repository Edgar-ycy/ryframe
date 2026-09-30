import { isDeepStrictEqual } from 'node:util'
import { createHash } from 'node:crypto'
import path from 'node:path'

const modes = new Set(['plan', 'apply', 'reconcile', 'verify'])
const idPattern = /^[1-9][0-9]{0,18}$/
const recordFields = ['ancestors', 'created_at', 'id', 'name', 'parent_id', 'remark', 'sort', 'status']
const templatePermission = 'system:user-import:add'

function exact(value, fields, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      !isDeepStrictEqual(Object.keys(value).sort(), [...fields].sort())) throw new Error(`${label}字段不完整`)
}

function safeId(value) {
  if (typeof value !== 'string' || !idPattern.test(value) || BigInt(value) > 9223372036854775807n)
    throw new Error('部门必须使用明确字符串ID')
  return value
}

export function departmentTargets(plan) {
  const tenants = plan?.environment?.tenants
  const groups = plan?.groups
  if (!Array.isArray(tenants) || tenants.length !== 10 || !Array.isArray(groups) || groups.length !== 11)
    throw new Error('部门阶段必须绑定十个普通租户的身份计划')
  const targets = tenants.map((tenant, index) => {
    const slot = `tenant-${String(index + 1).padStart(2, '0')}`
    const group = groups[index + 1]
    const templateUser = group?.users?.[0]
    if (tenant.slot !== slot || group?.slot !== slot || group.kind !== 'tenant' ||
        group.tenant_id !== tenant.tenant_id || !group.admin ||
        Object.entries(group.admin).some(([key, value]) => tenant.admin?.[key] !== value) ||
        !Array.isArray(group.permissions) || group.permissions.includes('*') ||
        !group.permissions.includes(templatePermission) || !templateUser)
      throw new Error('部门租户与身份组不一致')
    exact(templateUser, ['user_slot', 'username', 'password_env', 'client_address'], '部门模板固定用户')
    return { slot, tenant_id: tenant.tenant_id, admin: structuredClone(group.admin),
      template_user: structuredClone(templateUser) }
  })
  if (new Set(targets.map((item) => item.tenant_id)).size !== 10) throw new Error('部门租户重复')
  return targets
}

export function departmentTemplateIdentities(plan, identities) {
  const targets = departmentTargets(plan)
  if (!Array.isArray(identities) || identities.length !== targets.length)
    throw new Error('部门模板身份必须精确覆盖十个普通租户')
  const subjects = new Set()
  return targets.map((target, index) => {
    const identity = identities[index]
    exact(identity, ['subject_id', 'tenant_id', 'username', 'password_env', 'client_address'], '部门模板身份')
    const expected = { tenant_id: target.tenant_id, username: target.template_user.username,
      password_env: target.template_user.password_env, client_address: target.template_user.client_address }
    if (!safeId(identity.subject_id) || Object.entries(expected).some(([key, value]) => identity[key] !== value) ||
        subjects.has(identity.subject_id)) throw new Error('部门模板身份与首个固定用户或账本主体不符')
    subjects.add(identity.subject_id)
    return { ...target, template_identity: structuredClone(identity) }
  })
}

export function departmentName(plan) {
  const planId = plan?.environment?.plan_id
  const name = `dv-${planId}-import`
  if (typeof planId !== 'string' || !/^[a-z0-9][a-z0-9-]{2,31}$/.test(planId) || name.length > 50)
    throw new Error('部门名称不能由当前身份计划安全生成')
  return name
}

export function departmentBody(plan) {
  return { name: departmentName(plan), parent_id: null, sort: 0 }
}

export function validateDepartmentGrant(args, request, plan) {
  exact(request, ['id', 'operation', 'backend', 'frontend', 'config', 'identity', 'artifacts', 'run_dir', 'attempt',
    'tenant_ids', 'template_identities'], '部门授权')
  if (!modes.has(args.mode) || request.operation !== 'initialize' || request.run_dir !== args.runDirectory ||
      request.attempt !== args.attempt || path.resolve(request.backend) !== path.resolve(plan.environment.backend_dir) ||
      path.resolve(request.backend) !== process.cwd() || request.frontend !== plan.environment.frontend_dir ||
      request.artifacts !== path.join(args.runDirectory, 'seed-runtime', `attempt-${String(args.attempt).padStart(4, '0')}`, 'pacing'))
    throw new Error('部门授权不属于当前run、attempt或固定源码')
  const expectedTenants = departmentTargets(plan).map((item) => item.tenant_id)
  if (!isDeepStrictEqual(request.tenant_ids, expectedTenants)) throw new Error('部门授权必须精确绑定十个普通租户')
  const templateIdentities = departmentTemplateIdentities(plan, request.template_identities)
  exact(request.identity, ['subject_id', 'tenant_id', 'username', 'password_env', 'client_address'], '系统准备身份')
  if (!safeId(request.identity.subject_id) || Object.entries(plan.environment.system_admin)
    .some(([key, value]) => request.identity[key] !== value)) throw new Error('部门授权的系统准备身份不符')
  const expected = { contract: { timeout_ms: 120000, pacing: plan.environment.pacing.contract }, bindings: {
    scope_id: plan.environment.scope_id, api_url: plan.environment.api_url,
    frontend_url: plan.environment.frontend_url, pacing: plan.environment.pacing.bindings } }
  const actual = structuredClone(request.config)
  for (const key of ['api_url', 'frontend_url']) {
    if (new URL(actual?.bindings?.[key]).href !== new URL(expected.bindings[key]).href) throw new Error('部门会话端点不同')
    actual.bindings[key] = expected.bindings[key]
  }
  if (!isDeepStrictEqual(actual, expected)) throw new Error('部门会话配置或节奏绑定不同')
  return { scope_id: expected.bindings.scope_id, tenant_ids: expectedTenants, department_name: departmentName(plan),
    template_principals: templateIdentities.map(({ template_identity: identity }) => ({
      subject_id: identity.subject_id, tenant_id: identity.tenant_id, username: identity.username,
    })) }
}

export function departmentAuthorization(response, identity, mode) {
  const auth = response?.data
  const required = mode === 'apply' ? ['system:dept:list', 'system:dept:add'] : ['system:dept:list']
  if (!auth || typeof auth.user?.id !== 'string' || !idPattern.test(auth.user.id) ||
      BigInt(auth.user.id) > 9223372036854775807n || auth.user.tenant_id !== identity.tenant_id ||
      auth.user.username !== identity.username || !Array.isArray(auth.permissions) || auth.permissions.includes('*') ||
      !(auth.is_super_admin === true || required.every((item) => auth.permissions.includes(item))))
    throw new Error('部门会话的租户管理员或权威权限不符')
  return { subject_id: auth.user.id, tenant_id: auth.user.tenant_id, username: auth.user.username,
    is_super_admin: auth.is_super_admin, permissions: auth.permissions }
}

export function departmentTemplateAuthorization(response, identity) {
  const auth = response?.data
  if (!auth || typeof auth.user?.id !== 'string' || safeId(auth.user.id) !== identity.subject_id ||
      auth.user.tenant_id !== identity.tenant_id || auth.user.username !== identity.username ||
      auth.is_super_admin !== false || !Array.isArray(auth.permissions) || auth.permissions.includes('*') ||
      !auth.permissions.includes(templatePermission))
    throw new Error('部门模板会话的固定用户或导入权限不符')
  return { subject_id: auth.user.id, tenant_id: auth.user.tenant_id, username: auth.user.username,
    is_super_admin: auth.is_super_admin, permissions: auth.permissions }
}

export function departmentRecord(value) {
  exact(value, recordFields, '部门响应')
  safeId(value.id)
  if (typeof value.name !== 'string' || !value.name || (value.parent_id !== null && !idPattern.test(value.parent_id)) ||
      typeof value.ancestors !== 'string' || !Number.isSafeInteger(value.sort) || typeof value.status !== 'string' ||
      (value.remark !== null && typeof value.remark !== 'string') ||
      typeof value.created_at !== 'string' || !Number.isFinite(Date.parse(value.created_at)))
    throw new Error('部门响应字段无效')
  return structuredClone(value)
}

function pageData(response) {
  const data = response?.data
  exact(data, ['items', 'max_page_size', 'page', 'page_size', 'total', 'total_pages'], '部门列表')
  const expectedPages = data?.total === 0 ? 0 : 1
  if (response?.code !== 200 || !Array.isArray(data.items) || !Number.isSafeInteger(data.total) ||
      !Number.isSafeInteger(data.total_pages) || data.total_pages !== expectedPages ||
      data.max_page_size !== 100 || data.total < 0 || data.page !== 1 || data.page_size !== 100 ||
      data.total !== data.items.length)
    throw new Error('部门列表不是完整第一页像')
  return data
}

export function departmentListImage(response, name) {
  const data = pageData(response)
  const records = data.items.map(departmentRecord)
  const desired = records.filter((item) => item.name === name)
  return { total: data.total, records, desired }
}

export function departmentTemplateResponse(value) {
  exact(value, ['base64', 'bytes', 'media_type', 'sha256'], '部门模板响应')
  if (!Number.isSafeInteger(value.bytes) || value.bytes <= 0 || value.bytes > 512 * 1024 ||
      typeof value.media_type !== 'string' ||
      !value.media_type.toLowerCase().startsWith('application/vnd.openxmlformats-officedocument.spreadsheetml.sheet') ||
      !/^[a-f0-9]{64}$/.test(value.sha256) || typeof value.base64 !== 'string')
    throw new Error('部门模板响应类型或边界无效')
  const content = Buffer.from(value.base64, 'base64')
  if (content.length !== value.bytes || content.toString('base64') !== value.base64 ||
      createHash('sha256').update(content).digest('hex') !== value.sha256)
    throw new Error('部门模板响应内容与摘要不符')
  return structuredClone(value)
}

export function classifyDepartmentList(response, name) {
  const image = departmentListImage(response, name)
  if (image.total === 0) return { state: 'before', image }
  if (image.total === 1 && image.desired.length === 1) {
    const item = image.desired[0]
    if (item.parent_id === null && item.ancestors === '0' && item.sort === 0 && item.status === '1' && item.remark === null)
      return { state: 'after', image }
  }
  return { state: 'mismatch', image }
}

export function validateDepartmentDetail(response, name, id) {
  if (response?.code !== 200) throw new Error('部门详情缺少成功envelope')
  const item = departmentRecord(response.data)
  if (item.id !== safeId(id) || item.name !== name || item.parent_id !== null || item.ancestors !== '0' ||
      item.sort !== 0 || item.status !== '1' || item.remark !== null) throw new Error('部门详情不同于固定后像')
  return item
}

export class DepartmentSession {
  constructor(mode, plan, templateIdentities, sessionFor) {
    if (!modes.has(mode) || typeof sessionFor !== 'function') throw new Error('部门模式或会话工厂无效')
    this.mode = mode
    this.plan = plan
    this.name = departmentName(plan)
    this.targets = new Map(departmentTemplateIdentities(plan, templateIdentities)
      .map((item) => [item.tenant_id, item]))
    this.sessionFor = sessionFor
    this.before = new Set()
    this.written = new Set()
    this.failed = false
  }

  async operation(request) {
    if (this.failed) throw new Error('部门会话已经失败，禁止继续请求或重放')
    try { return await this.execute(request) }
    catch (error) { this.failed = true; throw error }
  }

  async execute(request) {
    if (!request || typeof request !== 'object' || Array.isArray(request) || !this.targets.has(request.tenant_id))
      throw new Error('部门请求租户未登记')
    const target = this.targets.get(request.tenant_id)
    if (request.operation === 'list') {
      exact(request, ['id', 'operation', 'tenant_id', 'name'], '部门列表请求')
      if (request.name !== this.name) throw new Error('部门列表名称不同于固定计划')
      const value = await this.sessionFor(target, 'owner', this.mode)
      const response = await value.session.request({ operation: 'get_system_depts', query: { page: 1, page_size: 100 } })
      const observed = classifyDepartmentList(response, this.name)
      if (observed.image.desired.length === 0) this.before.add(request.tenant_id)
      return { response, authorization: value.authorization }
    }
    if (request.operation === 'template') {
      exact(request, ['id', 'operation', 'tenant_id'], '部门模板请求')
      const value = await this.sessionFor(target, 'template', this.mode)
      const response = await value.session.request({
        operation: 'get_system_users_import_template',
        binary: true,
      })
      return { response: departmentTemplateResponse(response), authorization: value.authorization }
    }
    if (request.operation === 'detail') {
      exact(request, ['id', 'operation', 'tenant_id', 'department_id'], '部门详情请求')
      safeId(request.department_id)
      const value = await this.sessionFor(target, 'owner', this.mode)
      const response = await value.session.request({ operation: 'get_system_depts_by_id', path: { id: request.department_id } })
      departmentRecord(response?.data)
      return { response, authorization: value.authorization }
    }
    if (request.operation === 'create') {
      exact(request, ['id', 'operation', 'tenant_id', 'body', 'idempotency_key'], '部门创建请求')
      if (this.mode !== 'apply' || this.written.has(request.tenant_id) || !this.before.has(request.tenant_id) ||
          !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(request.idempotency_key) ||
          !isDeepStrictEqual(request.body, departmentBody(this.plan))) throw new Error('只允许空前像后的单次固定部门创建')
      this.written.add(request.tenant_id)
      const value = await this.sessionFor(target, 'owner', this.mode)
      const response = await value.session.request({ operation: 'post_system_depts', body: request.body,
        idempotency_key: request.idempotency_key })
      validateDepartmentDetail(response, this.name, response?.data?.id)
      return { response, authorization: value.authorization }
    }
    throw new Error('部门请求operation未登记')
  }
}
