const namePattern = /^[a-z][a-z0-9-]{0,63}$/
const digestPattern = /^[a-f0-9]{64}$/
const addressPattern = /^198\.(18|19)\.(25[0-5]|2[0-4][0-9]|1?[0-9]{1,2})\.(25[0-5]|2[0-4][0-9]|1?[0-9]{1,2})$/

function record(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function keys(value, expected) {
  return record(value) && Object.keys(value).sort().join() === expected.slice().sort().join()
}

function poolSelection(config, poolName, count) {
  if (!Number.isSafeInteger(count) || count < 1 || count > 10000)
    throw new Error('身份池选中数量无效')
  if (Object.hasOwn(config.bindings ?? {}, 'identities'))
    throw new Error('旧 identities 结构不可使用，必须声明独立身份池')
  const pool = config.contract?.identity_pools?.[poolName]
  const bindings = config.bindings?.identity_pools?.[poolName]
  if (typeof poolName !== 'string' || !namePattern.test(poolName) || !keys(pool, [
    'roles_sha256', 'permissions_sha256', 'client_address_model', 'slots', 'selections',
  ]) || !digestPattern.test(pool.roles_sha256) || !digestPattern.test(pool.permissions_sha256) ||
      pool.client_address_model !== 'fixed-benchmark-per-user' || !Array.isArray(pool.slots) ||
      !Array.isArray(bindings) || !record(pool.selections)) {
    throw new Error('身份池必须固定角色权限摘要、逻辑槽位及客户地址模型')
  }
  const slots = pool.slots.slice(0, count)
  const selected = bindings.slice(0, count)
  if (slots.length !== count || selected.length !== count)
    throw new Error('身份池缺少实际选中的独立用户')
  const tenants = new Map(), reverseTenants = new Map(), distribution = {}
  const users = new Set(), addresses = new Set(), subjects = new Set()
  const identities = selected.map((identity, index) => {
    const slot = slots[index]
    if (!keys(slot, ['tenant_slot', 'user_slot', 'client_address']) ||
        typeof slot.tenant_slot !== 'string' || !namePattern.test(slot.tenant_slot) ||
        typeof slot.user_slot !== 'string' || !namePattern.test(slot.user_slot) ||
        typeof slot.client_address !== 'string' || !addressPattern.test(slot.client_address) ||
        slot.client_address.split('.').some((part) => String(Number(part)) !== part) ||
        !keys(identity, ['tenant_id', 'username', 'password_env']) ||
        typeof identity.tenant_id !== 'string' || !/^[a-zA-Z0-9_-]{1,64}$/.test(identity.tenant_id) ||
        typeof identity.username !== 'string' || !identity.username.trim() || identity.username.length > 64 ||
        typeof identity.password_env !== 'string' || !/^[A-Z][A-Z0-9_]*$/.test(identity.password_env)) {
      throw new Error('身份池槽位、固定地址或具体身份绑定无效')
    }
    const subject = JSON.stringify([identity.tenant_id, identity.username.trim().toLowerCase()])
    if (users.has(slot.user_slot) || addresses.has(slot.client_address) || subjects.has(subject))
      throw new Error('实际选中身份必须为独立用户、逻辑槽位与固定地址')
    if ((slot.tenant_slot === 'system') !== (identity.tenant_id === 'system') ||
        (tenants.has(slot.tenant_slot) && tenants.get(slot.tenant_slot) !== identity.tenant_id) ||
        (reverseTenants.has(identity.tenant_id) && reverseTenants.get(identity.tenant_id) !== slot.tenant_slot)) {
      throw new Error('逻辑租户与实际租户必须保持一一对应')
    }
    tenants.set(slot.tenant_slot, identity.tenant_id)
    reverseTenants.set(identity.tenant_id, slot.tenant_slot)
    distribution[slot.tenant_slot] = (distribution[slot.tenant_slot] ?? 0) + 1
    users.add(slot.user_slot); addresses.add(slot.client_address); subjects.add(subject)
    return { ...identity, client_address: slot.client_address }
  })
  const declared = pool.selections[String(count)]
  if (!record(declared) || Object.keys(declared).length !== tenants.size ||
      Object.entries(declared).some(([tenant, amount]) => !namePattern.test(tenant) ||
        !Number.isSafeInteger(amount) || amount < 1 || distribution[tenant] !== amount)) {
    throw new Error('实际前 N 身份的租户分布必须匹配固定负载声明')
  }
  return { identities, evidence: { identity_pool: poolName, count,
    tenant_distribution: distribution, roles_sha256: pool.roles_sha256,
    permissions_sha256: pool.permissions_sha256, slots: structuredClone(slots) } }
}

function workloadSelection(config, suite, workflowName, concurrency) {
  const workflows = config.contract?.workloads?.[suite]
  const matches = Array.isArray(workflows) ? workflows.filter((value) => value.name === workflowName) : []
  if (matches.length !== 1) throw new Error('必须唯一声明当前身份池所属场景')
  const selected = poolSelection(config, matches[0].identity_pool, concurrency)
  const distribution = selected.evidence.tenant_distribution
  if ((suite === 'tenants' || (suite === 'jobs' && workflowName === 'import')) &&
      (Object.keys(distribution).length !== 10 || Object.hasOwn(distribution, 'system') ||
        Object.values(distribution).some((value) => value !== concurrency / 10))) {
    throw new Error('实际并发用户必须均匀覆盖十个普通租户')
  }
  if (suite === 'jobs' && workflowName === 'schedule' &&
      (Object.keys(distribution).length !== 1 || distribution.system !== concurrency)) {
    throw new Error('当前调度目标只能使用 system 普通身份池')
  }
  return { ...selected, evidence: { workflow: workflowName, ...selected.evidence } }
}

export function selectWorkloadIdentities(config, suite, workflowName, concurrency) {
  return workloadSelection(config, suite, workflowName, concurrency).identities
}

export function selectHomepageIdentities(config, cycles) {
  return poolSelection(config, config.contract?.homepage?.identity_pool, cycles).identities
}

/** 只验证并生成脱敏选择收据，真实授权由准备阶段的认证上下文核验。 */
export function validateSuiteSelection(config, suite, concurrency) {
  let selections
  if (suite === 'homepage') {
    const selected = poolSelection(config, config.contract?.homepage?.identity_pool, config.contract?.cycles)
    selections = [{ workflow: 'homepage', ...selected.evidence }]
  } else {
    const workflows = config.contract?.workloads?.[suite]
    if (!Array.isArray(workflows) || !workflows.length) throw new Error('当前场景没有固定身份池')
    selections = workflows.map((workflow) => workloadSelection(config, suite, workflow.name, concurrency).evidence)
  }
  return { format_version: 1, suite, selections }
}
