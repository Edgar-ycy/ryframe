import { setTimeout as delay } from 'node:timers/promises'

export function metric(source, name) {
  const values = source.split('\n').filter((line) => line.startsWith(name + ' ') || line.startsWith(name + '{'))
    .map((line) => Number(line.trim().split(/\s+/).at(-1)))
  if (!values.length || values.some((value) => !Number.isFinite(value) || value < 0)) {
    throw new Error(`缺少有效的 ${name} 指标`)
  }
  return values.reduce((sum, value) => sum + value, 0)
}

export function metricsHeaders(bindings, environment = process.env) {
  const name = bindings.metrics_token_env
  if (name === undefined) return {}
  if (typeof name !== 'string' || !/^[A-Z][A-Z0-9_]+$/.test(name) ||
      !environment[name] || /[\r\n]/.test(environment[name])) {
    throw new Error('指标认证必须引用已设置且合法的环境变量')
  }
  return { Authorization: `Bearer ${environment[name]}` }
}

async function read(url, headers) {
  const response = await fetch(url, { headers, redirect: 'error', signal: AbortSignal.timeout(5000) })
  if (!response.ok) {
    await response.body?.cancel()
    throw new Error(`指标采集 HTTP ${response.status}`)
  }
  return response.text()
}

function databaseConnections(source) {
  const single = (selector) => {
    const lines = source.split('\n').filter((line) => line.startsWith(selector + ' '))
    const value = Number(lines[0]?.trim().split(/\s+/).at(-1))
    if (lines.length !== 1 || !Number.isFinite(value) || value < 0) {
      throw new Error(`缺少唯一有效的 ${selector} 指标`)
    }
    return value
  }
  // 连接数可能在采集失败时残留，必须同时验证本次数据库连接和 global_status 采集。
  if (single('mysql_up') !== 1 ||
      single('mysql_exporter_collector_success{collector="collect.global_status"}') !== 1) {
    throw new Error('数据库指标连接或 global_status 采集失败')
  }
  const connections = single('mysql_global_status_threads_connected')
  if (!Number.isSafeInteger(connections)) throw new Error('数据库连接数必须为有效整数')
  return connections
}

export function resourceCollector() {
  let initial
  let previous
  const result = { peak_resident_memory_bytes: 0, cpu_seconds: 0, peak_database_connections: 0,
    collector_failures: 0 }
  const collect = (services, databases) => {
    const current = services.map((source) => ({
      cpu: metric(source, 'ryframe_process_cpu_seconds_total'),
      started: metric(source, 'ryframe_process_start_time_seconds'),
      memory: metric(source, 'ryframe_process_resident_memory_bytes'),
    }))
    if (!current.length || !databases.length) throw new Error('缺少服务或数据库指标')
    const connections = databases.reduce((sum, source) => sum + databaseConnections(source), 0)
    if (previous && (previous.length !== current.length || current.some((value, index) =>
      value.started !== previous[index].started || value.cpu < previous[index].cpu))) {
      throw new Error('测量期间服务重启或单进程 CPU 计数回退')
    }
    initial ??= current
    previous = current
    result.cpu_seconds = current.reduce((sum, value, index) => sum + (value.cpu - initial[index].cpu), 0)
    result.peak_resident_memory_bytes = Math.max(current.reduce((sum, value) => sum + value.memory, 0), result.peak_resident_memory_bytes)
    result.peak_database_connections = Math.max(connections, result.peak_database_connections)
  }
  return { result, collect }
}

export async function observe(config) {
  let stopped = false
  const serviceHeaders = metricsHeaders(config.bindings)
  const { result, collect: record } = resourceCollector()
  const reported = new Set()
  const collect = async () => {
    try {
      const services = await Promise.all(Object.values(config.bindings.metrics_urls).map((url) => read(url, serviceHeaders)))
      const databases = await Promise.all(config.bindings.database_metrics_urls.map((url) => read(url)))
      record(services, databases)
    } catch (error) {
      result.collector_failures += 1
      // fetch 的异常可能包含端点或代理信息，只输出固定网络分类和本地校验说明。
      const reason = error instanceof TypeError ? '指标网络连接失败' :
        error?.name === 'TimeoutError' ? '指标请求超时' : error.message
      if (!reported.has(reason)) {
        reported.add(reason)
        console.error(`运行时指标采集失败：${reason}`)
      }
    }
  }
  await collect()
  const running = (async () => {
    while (!stopped) { await delay(250); if (!stopped) await collect() }
  })()
  return async () => { stopped = true; await running; await collect(); return result }
}
