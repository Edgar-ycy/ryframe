import path from 'node:path'
import { randomUUID } from 'node:crypto'
import { lstat, readFile } from 'node:fs/promises'
import { hash } from './config.mjs'
import { withSession, downloadIdentityTemplate } from './identity-client.mjs'
import { digest } from './identity-plan.mjs'

export function userId(value) {
  if (typeof value !== 'string' || !/^[1-9][0-9]{0,18}$/.test(value) || BigInt(value) > 9223372036854775807n)
    throw new Error('身份资源必须返回明确字符串ID')
  return value
}

export function sameSet(actual, expected) {
  return Array.isArray(actual) && new Set(actual).size === actual.length &&
    JSON.stringify(actual.slice().sort()) === JSON.stringify(expected.slice().sort())
}

export function permissionIds(tree, required) {
  const found = new Map()
  function visit(nodes) {
    if (!Array.isArray(nodes)) throw new Error('权限树结构无效')
    for (const item of nodes) {
      if (required.includes(item.code)) {
        if (found.has(item.code) || item.status !== '1') throw new Error('权限码重复或不可用')
        found.set(item.code, userId(item.id))
      }
      visit(item.children)
    }
  }
  visit(tree)
  if (found.size !== required.length) throw new Error('当前权限目录缺少最小权限')
  return required.map((code) => found.get(code))
}

export function ordinaryContext(value, group, user, id) {
  if (value?.user?.id !== id || value.user.tenant_id !== group.tenant_id || value.user.username !== user.username ||
      value.is_super_admin !== false || !sameSet(value.roles, [group.role_code]) ||
      !sameSet(value.permissions, group.permissions)) throw new Error('真实普通会话身份、租户或权限不匹配')
  return { user_id: id, roles_sha256: hash(value.roles.slice().sort()), permissions_sha256: hash(value.permissions.slice().sort()) }
}

export function preparedPools(identities) {
  const system = identities.filter((identity) => identity.tenant_slot === 'system')
  const tenants = identities.filter((identity) => identity.tenant_slot !== 'system').sort((a, b) =>
    a.user_slot.slice(-3).localeCompare(b.user_slot.slice(-3)) || a.tenant_slot.localeCompare(b.tenant_slot))
  const contract = {}, bindings = {}
  for (const [name, selected] of Object.entries({ system, tenants })) {
    if (selected.length !== 100 || new Set(selected.map((value) => value.roles_sha256)).size !== 1 ||
        new Set(selected.map((value) => value.permissions_sha256)).size !== 1) throw new Error('准备池数量或授权摘要不一致')
    contract[name] = { roles_sha256: selected[0].roles_sha256, permissions_sha256: selected[0].permissions_sha256,
      client_address_model: 'fixed-benchmark-per-user', slots: selected.map(({ tenant_slot, user_slot, client_address }) =>
        ({ tenant_slot, user_slot, client_address })), selections: {} }
    for (const count of [10, 50, 100]) {
      const distribution = {}
      for (const identity of selected.slice(0, count)) distribution[identity.tenant_slot] = (distribution[identity.tenant_slot] ?? 0) + 1
      contract[name].selections[String(count)] = distribution
    }
    bindings[name] = selected.map(({ tenant_id, username, password_env }) => ({ tenant_id, username, password_env }))
  }
  return { contract, bindings }
}

const sameJson = (actual, expected) => hash(actual) === hash(expected)
const sha256 = (value) => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value)

function exactObject(value, fields, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      Object.keys(value).sort().join() !== fields.slice().sort().join()) throw new Error(`${label}字段不完整`)
}

async function validateTemplate(item, group, root) {
  exactObject(item, ['tenant_slot', 'path', 'template_sha256', 'department_path', 'department_sha256'], '身份模板收据')
  const filename = path.resolve(item.path)
  const expectedRoot = path.resolve(root)
  const escapedSlot = group.slot.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  if (item.tenant_slot !== group.slot || !path.isAbsolute(item.path) || filename !== item.path ||
      path.dirname(filename) !== expectedRoot ||
      !new RegExp(`^template-${escapedSlot}-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-` +
        `[89ab][0-9a-f]{3}-[0-9a-f]{12}\\.xlsx$`).test(path.basename(filename)) ||
      !sha256(item.template_sha256) || typeof item.department_path !== 'string' || !item.department_path ||
      item.department_sha256 !== digest(item.department_path)) throw new Error('身份模板收据没有绑定当前根目录与部门路径')
  const metadata = await lstat(filename)
  if (metadata.isSymbolicLink() || !metadata.isFile() || digest(await readFile(filename)) !== item.template_sha256)
    throw new Error('身份模板实物与收据摘要不一致')
}

export async function validateVerifiedReceipt(receipt, plan, state, root) {
  exactObject(receipt, ['format_version', 'plan_sha256', 'scope_id', 'status', 'identities', 'identity_pools',
    'templates', 'message_audience', 'audience_sha256', 'homepage_user_slots'], '身份验证收据')
  if (receipt.format_version !== 1 || receipt.plan_sha256 !== plan.plan_sha256 ||
      receipt.scope_id !== plan.environment.scope_id || receipt.status !== 'verified')
    throw new Error('验证收据不属于当前身份计划')
  const identities = plan.groups.flatMap((group) => group.users.map((user) => ({
    tenant_slot: group.slot, user_slot: user.user_slot, tenant_id: group.tenant_id,
    username: user.username, client_address: user.client_address, password_env: user.password_env,
    user_id: userId(state.users[user.user_slot]), roles_sha256: hash([group.role_code]),
    permissions_sha256: hash(group.permissions.slice().sort()),
  })))
  for (const identity of receipt.identities ?? [])
    exactObject(identity, ['tenant_slot', 'user_slot', 'tenant_id', 'username', 'client_address', 'password_env',
      'user_id', 'roles_sha256', 'permissions_sha256'], '身份验证投影')
  const pools = preparedPools(identities)
  const audience = identities.filter((identity) => identity.tenant_slot === 'system').slice(0, 10)
    .map((identity) => ({ kind: 'user', target_id: identity.user_id }))
  const homepage = identities.filter((identity) =>
    identity.tenant_slot !== 'system' && identity.user_slot.endsWith('-001')).map((identity) => identity.user_slot)
  if (!sameJson(receipt.identities, identities) || !sameJson(receipt.identity_pools, pools) ||
      !sameJson(receipt.message_audience, audience) || receipt.audience_sha256 !== hash(audience) ||
      !sameJson(receipt.homepage_user_slots, homepage)) throw new Error('身份验证收据投影与完整账本不一致')
  const tenantGroups = plan.groups.filter((group) => group.kind === 'tenant')
  if (!Array.isArray(receipt.templates) || receipt.templates.length !== tenantGroups.length)
    throw new Error('身份验证收据缺少十租户模板')
  for (const [index, group] of tenantGroups.entries()) await validateTemplate(receipt.templates[index], group, root)
  if (new Set(receipt.templates.map((item) => item.path)).size !== tenantGroups.length ||
      new Set(receipt.templates.map((item) => item.department_path)).size !== 1)
    throw new Error('身份验证模板路径或完整部门路径不唯一')
  return receipt
}

export async function verifyPrepared(plan, ledger, context, dependencies = {}) {
  const identities = [], templates = []
  for (const group of plan.groups) {
    const roleId = userId(ledger.state.roles[group.slot])
    await withSession(context, group.admin, async (admin) => {
      const role = (await admin.request({ operation: 'get_system_roles_by_id', path: { id: roleId } })).data
      if (role.id !== roleId || role.code !== group.role_code || role.is_super !== 0 || role.status !== '1' || role.data_scope !== '1')
        throw new Error('已登记角色状态或归属变化')
      const tree = (await admin.request({ operation: 'get_system_perms_tree' })).data
      const expected = permissionIds(tree, group.permissions)
      const actual = (await admin.request({ operation: 'get_system_roles_by_id_permissions', path: { id: roleId } })).data
      if (!sameSet(actual, expected)) throw new Error('角色实际权限与计划不一致')
      for (const user of group.users) {
        const id = userId(ledger.state.users[user.user_slot])
        const detail = (await admin.request({ operation: 'get_system_users_by_id', path: { id } })).data
        if (detail.id !== id || detail.username !== user.username || detail.status !== '1' ||
            detail.roles?.length !== 1 || detail.roles[0].id !== roleId || detail.roles[0].is_super !== 0)
          throw new Error('已登记用户状态或角色变化')
      }
    })
    for (const [index, user] of group.users.entries()) {
      const id = userId(ledger.state.users[user.user_slot])
      await withSession(context, { tenant_id: group.tenant_id, ...user }, async (session) => {
        const authenticated = (await session.request({ operation: 'get_auth_context' })).data
        const authorization = ordinaryContext(authenticated, group, user, id)
        identities.push({ tenant_slot: group.slot, user_slot: user.user_slot, tenant_id: group.tenant_id,
          username: user.username, client_address: user.client_address, password_env: user.password_env, ...authorization })
        if (group.kind === 'tenant' && index === 0) {
          const file = path.join(ledger.root, `template-${group.slot}-${randomUUID()}.xlsx`)
          const sha256 = await (dependencies.downloadTemplate ?? downloadIdentityTemplate)(context, session, file)
          const receipt = await context.run(plan.environment.database.python, { operation: 'template', path: file, sha256 })
          if (receipt.template_sha256 !== sha256 || typeof receipt.department_path !== 'string' || !receipt.department_path)
            throw new Error('模板检查器绑定不一致')
          templates.push({ tenant_slot: group.slot, path: file, ...receipt })
        }
      })
    }
  }
  if (identities.length !== 200 || templates.length !== 10 || new Set(templates.map((item) => item.department_path)).size !== 1)
    throw new Error('200个身份或十租户模板完整部门路径不一致')
  const audience = identities.filter((identity) => identity.tenant_slot === 'system').slice(0, 10)
    .map((identity) => ({ kind: 'user', target_id: identity.user_id }))
  await context.inspectAfter()
  const receipt = { format_version: 1, plan_sha256: plan.plan_sha256, scope_id: plan.environment.scope_id,
    status: 'verified', identities, identity_pools: preparedPools(identities), templates,
    message_audience: audience, audience_sha256: hash(audience),
    homepage_user_slots: identities.filter((identity) => identity.tenant_slot !== 'system' && identity.user_slot.endsWith('-001'))
      .map((identity) => identity.user_slot) }
  return validateVerifiedReceipt(receipt, plan, ledger.state, ledger.root)
}
