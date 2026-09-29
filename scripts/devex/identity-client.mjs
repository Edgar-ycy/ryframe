import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { mkdir, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import path from 'node:path'
import { Session, operationCatalog } from './request.mjs'
import { digest } from './identity-plan.mjs'
import { pythonInvocation } from '../python_process.mjs'

const inspector = fileURLToPath(new URL('../devex_identity_environment.py', import.meta.url))
export function pythonIdentity(executable, request) {
  return new Promise((resolve, reject) => {
    const invocation = pythonInvocation({ script: inspector })
    const child = spawn(executable, invocation.argv, {
      windowsHide: true,
      env: invocation.env,
      stdio: ['pipe', 'pipe', 'ignore'],
    })
    const chunks = []
    let size = 0, failed = false
    const timer = setTimeout(() => { failed = true; child.kill() }, 60000)
    child.on('error', () => { clearTimeout(timer); reject(new Error('身份环境检查器启动失败')) })
    child.stdout.on('data', (chunk) => {
      size += chunk.length
      if (size > 1024 * 1024) { failed = true; child.kill() } else chunks.push(chunk)
    })
    child.stdin.on('error', () => { failed = true })
    child.on('close', (code) => {
      clearTimeout(timer)
      try {
        const response = JSON.parse(Buffer.concat(chunks).toString('utf8'))
        if (failed || code !== 0 || response.ok !== true) throw new Error()
        resolve(response.receipt)
      } catch { reject(new Error('身份环境或ownership核验失败')) }
    })
    child.stdin.end(JSON.stringify(request))
  })
}

export async function preparationContext(plan, ledger, phase, dependencies = {}) {
  const environment = plan.environment
  const run = dependencies.python ?? pythonIdentity
  const inspected = await run(environment.database.python, { operation: 'inspect', environment, groups: plan.groups, phase })
  if (inspected.scope_id !== environment.scope_id || inspected.server_uuid !== environment.database.server_uuid)
    throw new Error('身份检查收据绑定错误')
  const artifacts = path.join(ledger.root, `${phase}-${randomUUID()}`)
  await mkdir(artifacts)
  await writeFile(path.join(artifacts, 'environment.json'), JSON.stringify(inspected, null, 2) + '\n', { flag: 'wx' })
  const config = { contract: { timeout_ms: 120000, pacing: environment.pacing.contract }, bindings: {
    scope_id: environment.scope_id, api_url: environment.api_url, frontend_url: environment.frontend_url,
    pacing: environment.pacing.bindings } }
  const factory = dependencies.createPacing ?? (await import('./pacing.mjs')).createPacing
  const pacing = await factory(config, { backend: environment.backend_dir,
    runner_frontend: environment.frontend_dir, artifacts })
  let catalog
  try { catalog = await (dependencies.operationCatalog ?? operationCatalog)(environment.backend_dir) }
  catch (error) { await pacing.close(); throw error }
  const SessionType = dependencies.Session ?? Session
  return {
    config, catalog, run,
    async session(identity) {
      return new SessionType(config, catalog, identity, await pacing.createPreparationControls(identity))
    },
    async inspectAfter() {
      const after = await run(environment.database.python, { operation: 'inspect', environment, groups: plan.groups, phase: 'verify' })
      if (after.scope_id !== inspected.scope_id || after.server_uuid !== inspected.server_uuid ||
          JSON.stringify(after.api_process) !== JSON.stringify(inspected.api_process)) throw new Error('身份准备期间API进程或资源绑定变化')
      return after
    },
    async close() { await pacing.close() },
  }
}

export async function withSession(context, identity, action) {
  const session = await context.session(identity)
  let loggedIn = false
  try { await session.login(); loggedIn = true; return await action(session) }
  finally { if (loggedIn) await session.request({ operation: 'post_auth_logout' }) }
}

export async function downloadIdentityTemplate(context, session, destination) {
  const operation = context.catalog.get('get_system_users_import_template')
  if (!operation || operation.method !== 'GET') throw new Error('当前契约缺少导入模板GET')
  const complete = await session.beforeRequest({ operation: 'get_system_users_import_template', method: 'GET', route: operation.path })
  try {
    const response = await fetch(new URL(operation.path, context.config.bindings.api_url), { redirect: 'error',
      signal: AbortSignal.timeout(120000), headers: { Authorization: `Bearer ${session.token}`,
        'X-Tenant-Id': session.identity.tenant_id, 'X-Forwarded-For': session.identity.client_address } })
    if (!response.ok) { await response.body?.cancel(); throw new Error('真实模板下载失败') }
    const chunks = []
    let size = 0
    for await (const chunk of response.body) {
      size += chunk.length
      if (size > 16 * 1024 * 1024) throw new Error('模板超过限制')
      chunks.push(Buffer.from(chunk))
    }
    const bytes = Buffer.concat(chunks)
    await writeFile(destination, bytes, { flag: 'wx' })
    return digest(bytes)
  } finally { await complete?.() }
}
