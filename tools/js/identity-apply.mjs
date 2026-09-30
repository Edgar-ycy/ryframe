import { withSession } from './identity-client.mjs'
import { permissionIds, sameSet, userId } from './identity-verify.mjs'

const requiredAdmin = ['system:role:list', 'system:role:add', 'system:role:edit',
  'system:perm:list', 'system:user:list', 'system:user:add', 'system:user:edit']

export function activationSecret(response, environment, group) {
  const data = response?.data
  userId(data?.request_id)
  const url = new URL(data.reset_url, environment.frontend_url)
  const parts = new URLSearchParams(url.hash.slice(1))
  if (url.origin !== new URL(environment.frontend_url).origin || url.pathname !== '/reset-password' || url.search ||
      [...parts.keys()].sort().join() !== 'request_id,tenant_id,token' ||
      parts.get('tenant_id') !== group.tenant_id || parts.get('request_id') !== data.request_id ||
      !parts.get('token') || !Number.isFinite(Date.parse(data.expires_at)) || Date.parse(data.expires_at) <= Date.now())
    throw new Error('激活地址、租户或有效期与本次用户不一致')
  return { request_id: data.request_id, tenant_id: group.tenant_id, token: parts.get('token') }
}

export function expectedIdentityEntries(plan) {
  return plan.groups.flatMap((group) => [
    { operation: 'post_system_roles', slot: group.slot },
    { operation: 'put_system_roles_by_id_permissions', slot: group.slot },
    ...group.users.flatMap((user) => [
      { operation: 'post_system_users', slot: user.user_slot },
      { operation: 'post_system_users_by_id_password_reset_requests', slot: user.user_slot },
      { operation: 'post_auth_password_reset_complete', slot: user.user_slot },
    ]),
  ])
}

function identityState(plan, state, statuses, complete) {
  const expected = expectedIdentityEntries(plan)
  const fields = ['format_version', 'plan_sha256', 'status', 'entries', 'roles', 'users',
    ...(['verifying', 'verified'].includes(state?.status) ? ['publication_nonce'] : []),
    ...(state?.status === 'verified' ? ['verified_receipt'] : [])]
  if (!state || typeof state !== 'object' || Array.isArray(state) ||
      Object.keys(state).sort().join() !== fields.sort().join() || state.format_version !== 1 ||
      state.plan_sha256 !== plan.plan_sha256 || !statuses.includes(state.status) || !Array.isArray(state.entries) ||
      !state.roles || typeof state.roles !== 'object' || Array.isArray(state.roles) ||
      !state.users || typeof state.users !== 'object' || Array.isArray(state.users) || state.entries.length > expected.length ||
      (complete && state.entries.length !== expected.length))
    throw new Error('身份账本不是当前可恢复的准备状态')
  const roles = {}, users = {}
  const resetIds = new Set()
  for (const [index, entry] of state.entries.entries()) {
    const target = expected[index], reset = target.operation === 'post_system_users_by_id_password_reset_requests'
    const fields = ['operation', 'slot', 'phase', 'id', ...(reset ? ['request_id'] : [])]
    if (!entry || typeof entry !== 'object' || Array.isArray(entry) ||
        Object.keys(entry).sort().join() !== fields.sort().join() || entry.operation !== target.operation ||
        entry.slot !== target.slot || entry.phase !== 'confirmed') throw new Error('身份账本不是确定性已确认前缀')
    userId(entry.id)
    if (reset) {
      userId(entry.request_id)
      if (resetIds.has(entry.request_id)) throw new Error('身份账本密码请求ID重复')
      resetIds.add(entry.request_id)
    }
    if (entry.operation === 'post_system_roles') roles[entry.slot] = entry.id
    if (entry.operation === 'post_system_users') users[entry.slot] = entry.id
    if (entry.operation === 'put_system_roles_by_id_permissions' && entry.id !== roles[entry.slot])
      throw new Error('身份账本角色权限写入没有绑定已确认角色')
    if (['post_system_users_by_id_password_reset_requests', 'post_auth_password_reset_complete'].includes(entry.operation) &&
        entry.id !== users[entry.slot]) throw new Error('身份账本用户写入没有绑定已确认用户')
  }
  const sameRecord = (actual, expected) => Object.keys(actual).length === Object.keys(expected).length &&
    Object.entries(expected).every(([key, value]) => actual[key] === value)
  const ids = [...Object.values(roles), ...Object.values(users)]
  if (!sameRecord(state.roles, roles) || !sameRecord(state.users, users) || new Set(ids).size !== ids.length)
    throw new Error('身份账本资源映射与确认前缀不一致')
  if (['prepared', 'verifying', 'verified'].includes(state.status) && state.entries.length !== expected.length)
    throw new Error('prepared身份账本写入序列不完整')
  return { roles, users, expected }
}

export function validateCompleteIdentityState(plan, state, statuses = ['prepared', 'verifying', 'verified']) {
  const progress = identityState(plan, state, statuses, true)
  if (['verifying', 'verified'].includes(state.status) &&
      (typeof state.publication_nonce !== 'string' ||
       !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(state.publication_nonce)))
    throw new Error('身份账本没有绑定唯一发布会话')
  if (state.status === 'verified' &&
      (typeof state.verified_receipt !== 'string' || !/^verified-[a-f0-9-]{36}\.json$/.test(state.verified_receipt)))
    throw new Error('verified身份账本没有绑定确定性收据')
  return progress
}

function progressFor(plan, ledger) {
  const state = ledger.state
  const { roles, users, expected } = identityState(plan, state, ['applying', 'prepared'], false)
  let cursor = 0
  return {
    roles, users, expected,
    take(operation, slot) {
      const target = expected[cursor]
      if (!target || target.operation !== operation || target.slot !== slot) throw new Error('身份执行顺序偏离确定性计划')
      return state.entries[cursor++] ?? null
    },
    complete() {
      if (cursor !== expected.length || state.entries.length !== expected.length)
        throw new Error('身份执行未完成确定性写入序列')
    },
  }
}

async function preflightGroups(plan, context, progress) {
  for (const group of plan.groups) {
    await withSession(context, group.admin, async (session) => {
      const auth = (await session.request({ operation: 'get_auth_context' })).data
      if (auth?.user?.tenant_id !== group.tenant_id || auth.user.username !== group.admin.username ||
          (auth.is_super_admin !== true && !requiredAdmin.every((code) => auth.permissions?.includes(code))))
        throw new Error('准备管理员租户或必要权限不足')
      const roles = (await session.request({ operation: 'get_system_roles', query: { code: group.role_code, page: 1, page_size: 100 } })).data
      const users = (await session.request({ operation: 'get_system_users', query: { username: `dv-${plan.environment.plan_id}-`, page: 1, page_size: 100 } })).data
      const expectedRoles = Object.hasOwn(progress.roles, group.slot) ? 1 : 0
      const expectedUsers = group.users.filter((user) => Object.hasOwn(progress.users, user.user_slot)).length
      if (roles?.total !== expectedRoles || users?.total !== expectedUsers)
        throw new Error('计划名称对象不同于账本已确认前缀，拒绝同名接管')
      permissionIds((await session.request({ operation: 'get_system_perms_tree' })).data, group.permissions)
    })
  }
}

async function prepareRole(group, admin, ledger, progress) {
  const permissions = permissionIds((await admin.request({ operation: 'get_system_perms_tree' })).data, group.permissions)
  let roleEntry = progress.take('post_system_roles', group.slot)
  if (!roleEntry) {
    await ledger.mutation('post_system_roles', group.slot,
      () => admin.request({ operation: 'post_system_roles', body: { code: group.role_code, name: '运行时性能普通用户', sort: 0, data_scope: '1' } }),
      (response) => {
        const role = response?.data
        if (role?.is_super !== 0 || role.code !== group.role_code || role.status !== '1' || role.data_scope !== '1') throw new Error('角色响应不符')
        ledger.state.roles[group.slot] = userId(role.id)
        return { id: role.id }
      })
    roleEntry = ledger.state.entries.at(-1)
  }
  const id = userId(roleEntry.id)
  const role = (await admin.request({ operation: 'get_system_roles_by_id', path: { id } })).data
  if (role?.id !== id || role.code !== group.role_code || role.is_super !== 0 || role.status !== '1' || role.data_scope !== '1')
    throw new Error('已登记角色状态或归属变化')
  const permissionEntry = progress.take('put_system_roles_by_id_permissions', group.slot)
  if (!permissionEntry) await ledger.mutation('put_system_roles_by_id_permissions', group.slot,
    () => admin.request({ operation: 'put_system_roles_by_id_permissions', path: { id }, body: { perm_ids: permissions } }), () => ({ id }))
  else if (permissionEntry.id !== id) throw new Error('角色权限写入与已登记角色不一致')
  const actual = (await admin.request({ operation: 'get_system_roles_by_id_permissions', path: { id } })).data
  if (!sameSet(actual, permissions)) throw new Error('分配后权限不一致')
  return id
}

async function prepareUser(plan, group, user, roleId, admin, context, ledger, progress) {
  let userEntry = progress.take('post_system_users', user.user_slot)
  if (!userEntry) {
    await ledger.mutation('post_system_users', user.user_slot,
      () => admin.request({ operation: 'post_system_users', body: { username: user.username, nickname: '运行时性能普通用户', role_ids: [roleId] } }),
      (response) => {
        if (response?.data?.username !== user.username || response.data.status !== 'pending_activation') throw new Error('用户创建响应不符')
        const id = userId(response.data.id)
        ledger.state.users[user.user_slot] = id
        return { id }
      })
    userEntry = ledger.state.entries.at(-1)
  }
  const id = userId(userEntry.id)
  let secret = null
  const resetEntry = progress.take('post_system_users_by_id_password_reset_requests', user.user_slot)
  if (!resetEntry) {
    const reset = await ledger.mutation('post_system_users_by_id_password_reset_requests', user.user_slot,
      () => admin.request({ operation: 'post_system_users_by_id_password_reset_requests', path: { id }, body: { reason: '本计划隔离性能普通用户首次激活' } }),
      (response) => ({ id, request_id: activationSecret(response, plan.environment, group).request_id }))
    secret = activationSecret(reset, plan.environment, group)
  } else if (resetEntry.id !== id) throw new Error('密码请求与已登记用户不一致')
  const activationEntry = progress.take('post_auth_password_reset_complete', user.user_slot)
  if (!activationEntry) {
    if (!secret) throw new Error('密码请求已确认但激活秘密未落盘，需人工核对后恢复')
    const password = process.env[user.password_env]
    if (!password) throw new Error('普通用户密码环境变量缺失')
    const activation = await context.session({ tenant_id: group.tenant_id, ...user })
    await activation.request({ operation: 'get_auth_csrf' })
    await ledger.mutation('post_auth_password_reset_complete', user.user_slot,
      () => activation.request({ operation: 'post_auth_password_reset_complete', body: { ...secret, new_password: password } }), () => ({ id }))
  } else if (activationEntry.id !== id) throw new Error('激活写入与已登记用户不一致')
  const detail = (await admin.request({ operation: 'get_system_users_by_id', path: { id } })).data
  if (detail?.id !== id || detail.username !== user.username || detail.status !== '1' ||
      detail.roles?.length !== 1 || detail.roles[0].id !== roleId || detail.roles[0].is_super !== 0)
    throw new Error('已登记用户状态或角色变化')
}

export async function applyIdentities(plan, ledger, context) {
  for (const group of plan.groups) for (const user of group.users)
    if (!process.env[user.password_env]) throw new Error('必须在写入前提供所有普通用户密码环境变量')
  let progress
  try { progress = progressFor(plan, ledger) }
  catch (error) {
    if (ledger.state.status === 'applying' && ledger.state.entries?.some((entry) => entry?.phase === 'started')) {
      ledger.state.status = 'needs-reconciliation'
      await ledger.persist()
    }
    throw error
  }
  await preflightGroups(plan, context, progress)
  await context.inspectAfter()
  for (const group of plan.groups) {
    await withSession(context, group.admin, async (admin) => {
      const roleId = await prepareRole(group, admin, ledger, progress)
      for (const user of group.users) await prepareUser(plan, group, user, roleId, admin, context, ledger, progress)
    })
  }
  progress.complete()
  await context.inspectAfter()
  ledger.state.status = 'prepared'
  await ledger.persist()
  return { status: 'prepared', plan_sha256: plan.plan_sha256, entries: ledger.state.entries.length,
    roles: Object.keys(ledger.state.roles).length, users: Object.keys(ledger.state.users).length }
}
