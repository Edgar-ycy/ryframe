export const windowMarginMs = 250
const record = (value) => value && typeof value === 'object' && !Array.isArray(value)
const integer = (value) => Number.isSafeInteger(value) && value > 0 && value <= 86_400_000 + windowMarginMs
const addressPattern = /^198\.(18|19)\.(25[0-5]|2[0-4][0-9]|1?[0-9]{1,2})\.(25[0-5]|2[0-4][0-9]|1?[0-9]{1,2})$/

export function validatePacingContract(pacing) {
  if (!record(pacing) || Object.keys(pacing).sort().join() !==
      'authority_sha256,operation_intervals_ms,request_interval_ms,sample_prepare_wait_ms' ||
      !/^[a-f0-9]{64}$/.test(pacing.authority_sha256) || !integer(pacing.request_interval_ms) ||
      !integer(pacing.sample_prepare_wait_ms) || !record(pacing.operation_intervals_ms) ||
      Object.entries(pacing.operation_intervals_ms).some(([name, interval]) =>
        !/^[a-z][a-z0-9_]*$/.test(name) || !integer(interval))) {
    throw new Error('固定节奏必须声明真实限流摘要、请求/接口间隔与样本准备窗口')
  }
  return pacing
}

export function matchedRule(operation, rules) {
  return [`${operation.method} ${operation.path}`, operation.path, operation.method]
    .find((rule) => Object.hasOwn(rules, rule))
}

const spacing = (window, capacity) => Math.floor(window / capacity) + 1

export function validatePacingAuthority(pacing, authority, catalog) {
  validatePacingContract(pacing)
  if (authority?.format_version !== 1 || !record(authority.api?.rules) ||
      typeof authority.user?.enabled !== 'boolean') throw new Error('限流 authority 结构不完整')
  for (const value of [authority.global, authority.user, authority.login]) {
    if (!integer(value?.window_ms) || !Number.isSafeInteger(value?.capacity) || value.capacity < 1)
      throw new Error('限流 authority 的容量或窗口无效')
  }
  if (!integer(authority.api.window_ms) || Object.values(authority.api.rules).some((value) =>
    !Number.isSafeInteger(value) || value < 1)) throw new Error('接口限流 authority 无效')
  const minimum = Math.max(spacing(authority.global.window_ms, authority.global.capacity),
    authority.user.enabled ? spacing(authority.user.window_ms, authority.user.capacity) : 0)
  const window = Math.max(authority.global.window_ms, authority.api.window_ms, authority.login.window_ms,
    authority.user.enabled ? authority.user.window_ms : 0)
  if (pacing.request_interval_ms < minimum || pacing.sample_prepare_wait_ms < window + windowMarginMs)
    throw new Error('固定请求节奏或样本准备窗口不足以满足实际全局/用户限流')
  for (const name of Object.keys(pacing.operation_intervals_ms)) {
    if (!catalog.has(name)) throw new Error('节奏声明包含契约中不存在的 operation')
  }
  for (const rule of Object.keys(authority.api.rules)) {
    if (![...catalog.values()].some((operation) =>
      [`${operation.method} ${operation.path}`, operation.path, operation.method].includes(rule)))
      throw new Error('限流规则无法用当前契约明确关联操作')
  }
  for (const [name, operation] of catalog) {
    const rule = matchedRule(operation, authority.api.rules)
    const required = Math.max(rule ? spacing(authority.api.window_ms, authority.api.rules[rule]) : 0,
      name === 'post_auth_login' ? spacing(authority.login.window_ms, authority.login.capacity) : 0)
    if (Math.max(pacing.request_interval_ms, pacing.operation_intervals_ms[name] ?? 0) < required)
      throw new Error(`固定节奏不满足实际接口或登录限流：${name}`)
  }
}

export function fixedPacer(pacing, authority, catalog, { now, sleep, onWait = () => {} }) {
  const clients = new Map(), principals = new Map(), starts = new Map()
  return (identity, phase = 'measurement') => {
    const address = identity.client_address
    const principal = JSON.stringify([identity.tenant_id, identity.username.trim().toLowerCase()])
    if (!addressPattern.test(address || '') ||
        (principals.has(principal) && principals.get(principal) !== address))
      throw new Error('固定节奏必须保持同一身份的明确客户地址')
    principals.set(principal, address)
    if (!clients.has(address)) clients.set(address, Promise.resolve())
    return async ({ operation: name, method, route }, beforeSend = async () => {}) => {
      const operation = catalog.get(name)
      if (!operation || operation.method !== method || operation.path !== route)
        throw new Error('请求节奏缺少当前 operation 的精确契约事实')
      const next = clients.get(address).then(async () => {
        const interval = Math.max(pacing.request_interval_ms, pacing.operation_intervals_ms[name] ?? 0)
        const rule = matchedRule(operation, authority.api.rules)
        const keys = [[`ip:${address}`, pacing.request_interval_ms]]
        if (Object.hasOwn(pacing.operation_intervals_ms, name)) keys.push([`operation:${address}:${name}`, interval])
        if (authority.user.enabled) keys.push([`user:${principal}`, pacing.request_interval_ms])
        if (rule) keys.push([`rule:${address}:${rule}`, interval])
        if (name === 'post_auth_login') keys.push([`login:${principal}`, interval], [`login-ip:${address}`, interval])
        const wait = Math.max(0, ...keys.map(([key, gap]) => (starts.get(key) ?? -Infinity) + gap - now()))
        const before = now()
        while (now() < before + wait) {
          const previous = now()
          await sleep(Math.max(1, Math.ceil(before + wait - previous)))
          if (now() < previous) throw new Error('固定节奏等待期间时钟回退')
        }
        const finished = now()
        const completion = await beforeSend()
        const started = now()
        if (finished < before + wait || started < finished) throw new Error('固定节奏等待提前结束或时钟回退')
        for (const [key] of keys) starts.set(key, started)
        onWait({ phase, operation: name, requested_ms: wait, duration_ms: finished - before })
        return completion
      })
      clients.set(address, next.catch(() => {}))
      return next
    }
  }
}
