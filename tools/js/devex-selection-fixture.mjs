/** 构建无凭据的选择模型；真实身份准备不能使用本测试数据。 */
export function identityPool(count, { tenants = 1, system = false, selections = [count] } = {}) {
  const slots = Array.from({ length: count }, (_, index) => ({
    tenant_slot: system ? 'system' : `tenant-${index % tenants + 1}`,
    user_slot: `user-${index + 1}`,
    client_address: `198.18.${Math.floor(index / 250) + 10}.${index % 250 + 1}`,
  }))
  return {
    contract: { roles_sha256: 'a'.repeat(64), permissions_sha256: 'b'.repeat(64),
      client_address_model: 'fixed-benchmark-per-user', slots,
      selections: Object.fromEntries(selections.map((selected) => {
        const counts = {}
        for (const slot of slots.slice(0, selected)) counts[slot.tenant_slot] = (counts[slot.tenant_slot] ?? 0) + 1
        return [String(selected), counts]
      })) },
    binding: slots.map((slot, index) => ({ tenant_id: system ? 'system' : `actual-${slot.tenant_slot}`,
      username: `user-${index + 1}`, password_env: 'RYFRAME_SELECTION_TEST_PASSWORD' })),
  }
}

export function selectionConfig(pool, suite = 'api', workflow = 'list') {
  return { contract: { identity_pools: { readers: pool.contract },
    workloads: { [suite]: [{ name: workflow, identity_pool: 'readers', steps: [] }] } },
  bindings: { identity_pools: { readers: pool.binding } } }
}
