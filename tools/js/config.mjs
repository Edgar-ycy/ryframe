import { readFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import path from 'node:path'
import { validatePacingContract } from './pacing-model.mjs'

export function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical)
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, canonical(value[key])]),
    )
  }
  return value
}

export function hash(value) {
  return createHash('sha256')
    .update(JSON.stringify(canonical(value)))
    .digest('hex')
}

export function boundedInteger(value, min, max, name) {
  if (!Number.isSafeInteger(value) || value < min || value > max)
    throw new Error(`${name} 超出范围`)
  return value
}

export function httpUrl(value) {
  const url = new URL(value)
  if (
    !['http:', 'https:'].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  ) {
    throw new Error('测量端点必须为不含凭据、query 和 fragment 的 HTTP URL')
  }
  return url
}

export function validateMetricsBindings(bindings) {
  const metrics = bindings.metrics_urls
  if (!metrics || Array.isArray(metrics) || typeof metrics !== 'object' ||
      Object.keys(metrics).sort().join() !== 'api,worker') {
    throw new Error('进程指标必须且只能登记 api 与 worker')
  }
  const processes = [httpUrl(metrics.api).href, httpUrl(metrics.worker).href]
  if (new Set(processes).size !== 2) throw new Error('API 与 Worker 指标端点规范化后不能重复')
  const databases = bindings.database_metrics_urls
  if (!Array.isArray(databases) || !databases.length)
    throw new Error('必须登记隔离 MySQL 实例的连接数指标端点')
  const normalized = databases.map((url) => httpUrl(url).href)
  if (new Set(normalized).size !== normalized.length)
    throw new Error('数据库指标端点规范化后不能重复')
}

export async function configuration(backend, suite, environment = process.env) {
  const config = JSON.parse(
    await readFile(path.join(backend, '.local-tests/devex/runtime.json'), 'utf8'),
  )
  const { contract, bindings } = config
  if (!contract || !bindings || config.schema_version !== 1) throw new Error('运行时配置结构无效')
  if (!/^[a-z0-9][a-z0-9-]{2,63}$/.test(bindings.scope_id)) throw new Error('必须登记隔离 scope')
  if (
    !/^[a-f0-9]{64}$/.test(contract.dataset_sha256) ||
    !/^[a-f0-9]{64}$/.test(contract.environment_sha256)
  ) {
    throw new Error('缺少固定数据集或硬件、存储、网络与服务参数指纹')
  }
  if (
    hash(contract) !== environment.RYFRAME_DEVEX_RUNTIME_INPUT_SHA256 ||
    JSON.stringify(canonical(bindings.source_fingerprints)) !==
      JSON.stringify(canonical(JSON.parse(environment.RYFRAME_DEVEX_SOURCE_FINGERPRINTS)))
  ) {
    throw new Error('测量配置与本次源码或负载指纹不一致')
  }
  for (const url of [
    bindings.api_url,
    bindings.frontend_url,
  ])
    httpUrl(url)
  validateMetricsBindings(bindings)
  validatePacingContract(contract.pacing)
  boundedInteger(contract.cycles, 10, 10000, '每个场景的请求周期')
  boundedInteger(contract.timeout_ms, 1000, 300000, '请求超时')
  if (Object.hasOwn(bindings, 'identities') || !bindings.identity_pools ||
      Array.isArray(bindings.identity_pools) || typeof bindings.identity_pools !== 'object' ||
      !Object.keys(bindings.identity_pools).length)
    throw new Error('缺少独立身份池或仍使用旧 identities 结构')
  const required = {
    api: ['list', 'filter', 'page', 'login-refresh', 'write'],
    jobs: ['export', 'import', 'message', 'schedule'],
    tenants: ['list', 'write', 'export'],
  }
  if (suite !== 'homepage') {
    const workflows = contract.workloads[suite]
    if (
      !Array.isArray(workflows) ||
      workflows.length !== required[suite]?.length ||
      new Set(workflows.map((v) => v.name)).size !== workflows.length ||
      !required[suite].every((name) => workflows.some((v) => v.name === name))
    ) {
      throw new Error('运行时场景集合不完整或重复')
    }
    for (const workflow of workflows) {
      if (!Array.isArray(workflow.steps) || !workflow.steps.length || workflow.steps.length > 30) {
        throw new Error('场景必须包含 1 到 30 个实际操作')
      }
      if (suite === 'jobs' && workflow.job?.kind !== workflow.name) {
        throw new Error('任务场景必须声明同种类的精确关联任务采集')
      }
      if (
        workflow.job &&
        (!bindings.job_timings || !path.isAbsolute(bindings.job_timings.python))
      ) {
        throw new Error('任务场景缺少显式 Python 与隔离控制库采集绑定')
      }
    }
  }
  return config
}

export function pointer(value, key) {
  if (!key.startsWith('/')) throw new Error('结果引用必须使用 JSON Pointer')
  for (const part of key.slice(1).split('/')) {
    const name = part.replaceAll('~1', '/').replaceAll('~0', '~')
    if (!value || typeof value !== 'object' || !Object.hasOwn(value, name))
      throw new Error('响应缺少预期字段')
    value = value[name]
  }
  return value
}

export function expand(value, variables) {
  if (typeof value === 'string') {
    const match = /^\$\{([a-zA-Z0-9_]+)\}$/.exec(value)
    if (match) {
      if (!Object.hasOwn(variables, match[1])) throw new Error(`缺少场景变量 ${match[1]}`)
      return variables[match[1]]
    }
    return value.replace(/\$\{([a-zA-Z0-9_]+)\}/g, (_, key) => {
      if (!Object.hasOwn(variables, key)) throw new Error(`缺少场景变量 ${key}`)
      return String(variables[key])
    })
  }
  if (Array.isArray(value)) return value.map((v) => expand(v, variables))
  if (value && typeof value === 'object')
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, expand(v, variables)]))
  return value
}
