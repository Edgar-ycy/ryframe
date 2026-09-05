import readline from 'node:readline'
import { pathToFileURL } from 'node:url'
import path from 'node:path'

const args = process.argv.slice(2)
if (args.length !== 4 || args[0] !== '--run-dir' || args[2] !== '--attempt'
    || !path.isAbsolute(args[1]) || !/^[1-9][0-9]*$/.test(args[3]))
  throw new Error('必须明确唯一 run 和 attempt')
const runDirectory = args[1], attempt = Number(args[3])
// 唯一控制通道是本地 stdin；不监听网络，不打印 cookie、token 或登录响应。
const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity })
let session, pacing, loggedOut = false
async function closeSession() {
  let timer
  try {
    if (session && !loggedOut) {
      await Promise.race([session.request({ operation: 'post_auth_logout' }),
        new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('注销超时')), 10000) })])
      loggedOut = true
    }
  } finally { clearTimeout(timer); await pacing?.close(); pacing = undefined }
}
for await (const line of input) {
  let id
  try {
    if (Buffer.byteLength(line) > 1024 * 1024) throw new Error('输入过大')
    const request = JSON.parse(line)
    id = request.id
    let data
    if (request.operation === 'initialize' && !session) {
      if (request.run_dir !== runDirectory || request.attempt !== attempt)
        throw new Error('首个会话动作不属于登记 run 和 attempt')
      const { Session, operationCatalog } = await import(pathToFileURL(path.join(request.backend, 'scripts/devex/request.mjs')))
      const { createPacing } = await import(pathToFileURL(path.join(request.backend, 'scripts/devex/pacing.mjs')))
      const config = request.config
      pacing = await createPacing(config, { backend: request.backend, frontend: request.frontend, artifacts: request.artifacts })
      session = new Session(config, await operationCatalog(request.backend), request.identity,
        await pacing.createPreparationControls(request.identity))
      await session.login()
      const response = await session.request({ operation: 'get_auth_context' })
      const auth = response.data
      data = { subject_id: auth.user?.id, tenant_id: auth.user?.tenant_id,
        username: auth.user?.username, is_super_admin: auth.is_super_admin, permissions: auth.permissions }
    } else if (request.operation === 'get' && session) {
      data = (await session.request({ operation: 'get_monitor_schedules_by_id', path: { id: request.schedule_id } })).data
    } else if (request.operation === 'disable' && session) {
      data = (await session.request({ operation: 'put_monitor_schedules_by_id_status',
        path: { id: request.schedule_id }, body: { enabled: false, version: request.version } })).data
    } else if (request.operation === 'close' && session) {
      await closeSession()
      data = { closed: true }
    } else throw new Error('未知或顺序错误的 operation')
    process.stdout.write(JSON.stringify({ id, ok: true, data }) + '\n')
    if (request.operation === 'close') break
  } catch (error) {
    process.stdout.write(JSON.stringify({ id, ok: false, error_type: error?.name ?? 'Error',
      status: Number.isInteger(error?.status) ? error.status : null }) + '\n')
    break
  }
}
try { await closeSession() } catch { process.exitCode = 1 }
