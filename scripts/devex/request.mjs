import { readFile } from 'node:fs/promises'
import { createHash, randomUUID } from 'node:crypto'
import { setTimeout as delay } from 'node:timers/promises'
import path from 'node:path'
import { expand, pointer } from './config.mjs'
import { RequestFailure } from './failure.mjs'

export function operationCatalogDocument(document) {
  const schema = JSON.parse(Buffer.isBuffer(document) ? document.toString('utf8') : document)
  const operations = new Map()
  const prefix = schema['x-ryframe-api-prefix']?.value
  if (typeof prefix !== 'string' || !prefix.startsWith('/') || prefix.startsWith('//')) {
    throw new Error('契约缺少明确的 API 前缀')
  }
  for (const [route, item] of Object.entries(schema.paths)) {
    for (const [method, op] of Object.entries(item)) {
      if (!op.operationId) continue
      if (operations.has(op.operationId)) throw new Error('重复 operation ID')
      if (!route.startsWith('/') || route.startsWith('//') || /[?#]/.test(route)) {
        throw new Error('operation 必须声明当前服务的绝对路径')
      }
      operations.set(op.operationId, { method: method.toUpperCase(), path: route })
    }
  }
  return operations
}

export async function operationCatalog(backend) {
  return operationCatalogDocument(await readFile(path.join(backend, 'openapi/openapi.json')))
}

export class Session {
  constructor(config, catalog, identity, controls = {}) {
    if (
      identity.client_address !== undefined &&
      (!/^198\.(18|19)\.(25[0-5]|2[0-4][0-9]|1?[0-9]{1,2})\.(25[0-5]|2[0-4][0-9]|1?[0-9]{1,2})$/.test(
        identity.client_address,
      ) ||
        !['127.0.0.1', '[::1]'].includes(new URL(config.bindings.api_url).hostname))
    ) {
      throw new Error('测试客户地址只允许明确 loopback API 与 198.18/19 基准地址段')
    }
    this.config = config
    this.catalog = catalog
    this.identity = identity
    this.cookies = new Map()
    this.token = undefined
    this.requests = 0
    if (controls.beforeRequest !== undefined && typeof controls.beforeRequest !== 'function') {
      throw new Error('请求准备 hook 必须是函数')
    }
    this.beforeRequest = controls.beforeRequest ?? (async () => {})
    if (controls.multipartSample !== undefined && typeof controls.multipartSample !== 'function') {
      throw new Error('multipart 样本读取器必须是函数')
    }
    this.multipartSample = controls.multipartSample
  }

  async login() {
    const password = process.env[this.identity.password_env]
    if (!password || !this.identity.username || !this.identity.tenant_id)
      throw new Error('缺少显式身份或密码环境变量')
    await this.request({ operation: 'get_auth_csrf' })
    const body = await this.request({
      operation: 'post_auth_login',
      body: {
        username: this.identity.username,
        password,
      },
    })
    this.token = pointer(body, '/data/access_token')
  }

  async request(step) {
    const operation = this.catalog.get(step.operation)
    if (!operation) throw new Error(`契约中不存在 ${step.operation}`)
    if (step.poll && operation.method !== 'GET') throw new Error('只允许轮询只读 operation')
    if (
      step.binary !== undefined &&
      (step.binary !== true ||
        operation.method !== 'GET' ||
        step.operation !== 'get_system_users_import_template')
    ) {
      throw new Error('只允许捕获当前用户导入模板的受限二进制响应')
    }
    // 刷新会话要求挑战绑定当前 sid，不能复用登录前的匿名挑战。
    if (['post_auth_refresh', 'post_auth_logout'].includes(step.operation)) {
      await this.request({ operation: 'get_auth_csrf' })
    }
    let route = operation.path.replace(/\{([^}]+)\}/g, (_, key) => {
      if (step.path?.[key] === undefined) throw new Error('路径参数缺失')
      return encodeURIComponent(String(step.path[key]))
    })
    const url = new URL(route, this.config.bindings.api_url)
    for (const [key, value] of Object.entries(step.query || {})) {
      if (value !== undefined && value !== null) url.searchParams.set(key, String(value))
    }
    if (
      step.idempotency_key !== undefined &&
      (step.operation !== 'post_system_depts' ||
        typeof step.idempotency_key !== 'string' ||
        !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(
          step.idempotency_key,
        ))
    ) {
      throw new Error('仅固定部门创建可传入规范幂等键')
    }
    const headers = {
      'X-Tenant-Id': this.identity.tenant_id,
      'Idempotency-Key': step.idempotency_key ?? randomUUID(),
      Origin: new URL(this.config.bindings.frontend_url).origin,
    }
    if (this.identity.client_address !== undefined)
      headers['X-Forwarded-For'] = this.identity.client_address
    if (this.token) headers.Authorization = `Bearer ${this.token}`
    if (this.cookies.size)
      headers.Cookie = [...this.cookies].map(([key, value]) => `${key}=${value}`).join('; ')
    if (this.cookies.has('ryframe_csrf')) headers['x-csrf-token'] = this.cookies.get('ryframe_csrf')
    let body
    if (step.multipart) {
      body = new FormData()
      for (const file of step.multipart) {
        const content = await this.multipartContent(file)
        body.set(file.field, new Blob([content], { type: file.type }), file.filename)
      }
    } else if (step.body !== undefined) {
      headers['Content-Type'] = 'application/json'
      body = JSON.stringify(step.body)
    }
    const complete = await this.beforeRequest({ operation: step.operation, method: operation.method, route: operation.path })
    if (complete !== undefined && typeof complete !== 'function') throw new Error('请求完成 hook 必须是函数')
    return finishRequest(complete, async () => {
      this.requests++
      const response = await fetch(url, {
        method: operation.method,
        headers,
        body,
        redirect: 'error',
        signal: AbortSignal.timeout(this.config.contract.timeout_ms),
      })
      for (const cookie of response.headers.getSetCookie()) {
        const pair = cookie.split(';')[0]
        const separator = pair.indexOf('=')
        this.cookies.set(pair.slice(0, separator), pair.slice(separator + 1))
      }
      if (!response.ok) {
        await response.body?.cancel()
        throw new RequestFailure('http_status', step.operation, response.status)
      }
      if (response.status === 204) return null
      const media = response.headers.get('content-type') || ''
      let result
      try {
        result = step.binary
          ? await readBinaryResponse(response, media)
          : await readResponse(response, media.includes('json'))
      } catch {
        throw new RequestFailure('invalid_response', step.operation)
      }
      if (result && typeof result === 'object' && result.code !== undefined && result.code !== 200) {
        throw new RequestFailure('business_response', step.operation)
      }
      if (step.operation === 'post_auth_refresh') this.token = pointer(result, '/data/access_token')
      return result
    })
  }

  async multipartContent(file) {
    if (Object.hasOwn(file, 'sample') === Object.hasOwn(file, 'path_env')) {
      throw new Error('multipart 必须且只能选择 sample 或 path_env')
    }
    if (Object.hasOwn(file, 'sample')) {
      if (typeof file.sample !== 'string' || !this.multipartSample) throw new Error('缺少本次 multipart 样本读取器')
      return this.multipartSample(file.sample)
    }
    if (typeof file.path_env !== 'string' || !/^[A-Z][A-Z0-9_]*$/.test(file.path_env) || !process.env[file.path_env]) {
      throw new Error('缺少显式上传样本路径')
    }
    return readFile(process.env[file.path_env])
  }

  async workflow(workflow, variables) {
    let final
    for (const definition of workflow.steps) {
      const step = expand(definition, variables)
      if (step.operation === 'post_auth_login') {
        await this.login()
        continue
      }
      const started = performance.now()
      do {
        final = await this.request(step)
        if (!step.poll) break
        const state = pointer(final, step.poll.pointer)
        if (state === step.poll.equals) break
        if (
          step.poll.failures?.includes(state) ||
          performance.now() - started >= this.config.contract.timeout_ms
        ) {
          throw new RequestFailure(
            step.poll.failures?.includes(state) ? 'poll_failed' : 'poll_timeout',
            step.operation,
          )
        }
        await delay(250)
      } while (true)
      for (const [key, at] of Object.entries(step.save || {})) variables[key] = pointer(final, at)
      for (const assertion of step.expect || []) {
        if (
          JSON.stringify(pointer(final, assertion.pointer)) !== JSON.stringify(assertion.equals)
        ) {
          throw new Error(`${workflow.name}: 响应断言失败`)
        }
      }
    }
    return final
  }
}

async function finishRequest(complete, request) {
  let result, failure
  try { result = await request() } catch (error) { failure = error }
  try { await complete?.() }
  catch (error) { failure = failure ? new AggregateError([failure, error], `${failure.message}; ${error.message}`) : error }
  if (failure) throw failure
  return result
}

export async function readResponse(response, json) {
  const chunks = []
  let bytes = 0
  for await (const chunk of response.body || []) {
    bytes += chunk.byteLength
    if (json) {
      if (bytes > 16 * 1024 * 1024) throw new Error('负载响应 JSON 超过 16 MiB 上限')
      chunks.push(Buffer.from(chunk))
    }
  }
  if (!json) return { bytes }
  return JSON.parse(Buffer.concat(chunks).toString('utf8'))
}

export async function readBinaryResponse(response, mediaType) {
  const chunks = []
  let bytes = 0
  for await (const chunk of response.body || []) {
    bytes += chunk.byteLength
    if (bytes > 512 * 1024) throw new Error('受限二进制响应超过 512 KiB 上限')
    chunks.push(Buffer.from(chunk))
  }
  if (!bytes || typeof mediaType !== 'string' || mediaType.toLowerCase().includes('json')) {
    throw new Error('受限二进制响应为空或媒体类型错误')
  }
  const content = Buffer.concat(chunks)
  return {
    bytes,
    media_type: mediaType,
    sha256: createHash('sha256').update(content).digest('hex'),
    base64: content.toString('base64'),
  }
}
