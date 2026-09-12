import { readFile, writeFile, mkdir } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { createIdentityPlan, localPath, validateIdentityPlan } from './devex/identity-plan.mjs'
import { identityLedger } from './devex/identity-ledger.mjs'
import { preparationContext } from './devex/identity-client.mjs'
import { applyIdentities } from './devex/identity-apply.mjs'
import { verifyPrepared } from './devex/identity-verify.mjs'
import { strictScalarJsonObject } from './private_json_protocol.mjs'

const protocolKey = 'RYFRAME_PERFORMANCE_IDENTITIES_PROTOCOL'
const protocolPrefix = 'RYFRAME_PERFORMANCE_IDENTITIES_'

export class IdentityProtocolError extends Error {}

function exactProtocol(value, fields) {
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      JSON.stringify(Object.keys(value).sort()) !== JSON.stringify(fields.slice().sort()))
    throw new IdentityProtocolError('私有协议字段不完整或含未知字段')
}

function protocolPath(value, label) {
  if (typeof value !== 'string' || !value || !path.isAbsolute(value))
    throw new IdentityProtocolError(`${label}必须是非空绝对路径`)
  if (/[\n\r\0]/u.test(value)) throw new IdentityProtocolError(`${label}不能包含换行符或NUL`)
  return value
}

export function privateProtocolArguments(argv = process.argv.slice(2), environment = process.env) {
  if (argv.length) throw new IdentityProtocolError('身份准备脚本是私有实现，不接受命令行参数')
  const unknown = Object.keys(environment)
    .filter((name) => name.startsWith(protocolPrefix) && name !== protocolKey)
  if (unknown.length) throw new IdentityProtocolError(`身份准备私有环境含未知字段：${unknown.sort().join(',')}`)
  const protocol = strictScalarJsonObject(
    environment[protocolKey],
    IdentityProtocolError,
    '身份准备私有协议',
  )
  if (protocol?.format_version !== 1 || typeof protocol.operation !== 'string')
    throw new IdentityProtocolError('身份准备私有协议版本或操作无效')
  if (protocol.operation === 'plan') {
    exactProtocol(protocol, ['format_version', 'operation', 'environment', 'output', 'write'])
    if (protocol.write !== true) throw new IdentityProtocolError('身份计划必须显式授权写入')
    return ['plan', '--environment', protocolPath(protocol.environment, '环境清单'),
      '--output', protocolPath(protocol.output, '身份计划'), '--write']
  }
  if (!['apply', 'verify'].includes(protocol.operation))
    throw new IdentityProtocolError('身份准备私有协议操作无效')
  exactProtocol(protocol, ['format_version', 'operation', 'plan', 'state_dir', 'write'])
  if (protocol.write !== true) throw new IdentityProtocolError('身份应用或核验必须显式授权写入')
  return [protocol.operation, '--plan', protocolPath(protocol.plan, '身份计划'),
    '--state-dir', protocolPath(protocol.state_dir, '身份账本目录'), '--write']
}

export async function executeIdentityPlan(plan, directory, mode, dependencies = {}) {
  validateIdentityPlan(plan)
  if (!['apply', 'verify'].includes(mode)) throw new Error('未知身份准备阶段')
  const current = await createIdentityPlan(plan.environment)
  if (current.plan_sha256 !== plan.plan_sha256) throw new Error('计划与确定性账号模型不同')
  const ledger = await identityLedger(plan, directory, mode, dependencies.publicationCheckpoint,
    dependencies.lockOwnerToken)
  let context
  let contextClosed = false
  try {
    if (mode === 'apply') await ledger.requireApply()
    else {
      const recovered = await ledger.resumeVerified()
      if (recovered) return verifiedSummary(plan, recovered)
      await ledger.beginVerification()
    }
    context = await preparationContext(plan, ledger, mode, dependencies)
    const receipt = mode === 'apply' ? await applyIdentities(plan, ledger, context)
      : await verifyPrepared(plan, ledger, context, dependencies)
    contextClosed = true
    await context.close()
    if (mode === 'apply') return receipt
    await ledger.publishVerified(receipt)
    return verifiedSummary(plan, receipt)
  } finally {
    try { if (!contextClosed) await context?.close() } finally { await ledger.close() }
  }
}

function verifiedSummary(plan, receipt) {
  return { status: receipt.status, plan_sha256: plan.plan_sha256, users: receipt.identities.length,
    tenants: receipt.templates.length, message_audience: receipt.message_audience.length }
}

export async function main(argv, dependencies = {}) {
  const [command, ...values] = argv
  if (command === '--help') {
    process.stdout.write('身份准备：plan --environment <环境清单> --output <计划> --write；apply|verify --plan <计划> --state-dir <账本目录> --write\n')
    return
  }
  const options = new Map()
  for (let index = 0; index < values.length; index++) {
    const name = values[index]
    if (options.has(name) || !['--environment', '--output', '--plan', '--state-dir', '--write'].includes(name)) throw new Error('命令参数无效')
    options.set(name, name === '--write' ? true : values[++index])
  }
  if (command === 'plan') {
    if (options.size !== 3 || !options.has('--environment') || !options.has('--output') ||
        options.get('--write') !== true) throw new Error('plan需要环境清单、输出和显式--write')
    const environment = JSON.parse(await readFile(await localPath(process.cwd(), options.get('--environment')), 'utf8'))
    if (path.resolve(environment.backend_dir) !== process.cwd()) throw new Error('必须在目标后端目录运行')
    const plan = await createIdentityPlan(environment)
    const output = await localPath(process.cwd(), options.get('--output'), false)
    await mkdir(path.dirname(output), { recursive: true })
    await writeFile(output, JSON.stringify(plan, null, 2) + '\n', { flag: 'wx' })
    process.stdout.write(JSON.stringify({ status: 'planned', users: 200, plan_sha256: plan.plan_sha256 }) + '\n')
  } else {
    if (!['apply', 'verify'].includes(command) || options.size !== 3 || options.get('--write') !== true ||
        !options.has('--plan') || !options.has('--state-dir')) throw new Error('apply/verify需要明确计划、账本和--write')
    const plan = JSON.parse(await readFile(await localPath(process.cwd(), options.get('--plan')), 'utf8'))
    if (path.resolve(plan.environment.backend_dir) !== process.cwd()) throw new Error('必须在目标后端目录运行')
    process.stdout.write(JSON.stringify(await executeIdentityPlan(
      plan, options.get('--state-dir'), command, dependencies)) + '\n')
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const arguments_ = privateProtocolArguments()
    delete process.env[protocolKey]
    await main(arguments_)
  } catch (error) {
    if (error instanceof IdentityProtocolError) {
      process.stderr.write('identity_preparation_protocol_error：身份准备私有协议无效。\n')
      process.exitCode = 2
    } else {
      process.stderr.write('identity_preparation_failed：身份准备失败，请核对本计划账本；不自动接管或重放。\n')
      process.exitCode = 1
    }
  }
}
