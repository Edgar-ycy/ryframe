import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { waitForProducerStart } from './devex_clone_existing.mjs'

export function identityProducerArguments(argv) {
  const names = ['--run-dir', '--attempt', '--identity-plan', '--identity-state', '--identity-mode']
  const options = new Map()
  for (let index = 0; index < argv.length; index++) {
    const name = argv[index], value = argv[++index]
    if (!names.includes(name) || options.has(name) || !value || value.startsWith('--'))
      throw new Error('身份生产者参数未知、重复或缺少值')
    options.set(name, value)
  }
  if (options.size !== names.length
      || ['--run-dir', '--identity-plan', '--identity-state'].some((key) => !path.isAbsolute(options.get(key)))
      || !/^[1-9][0-9]*$/.test(options.get('--attempt'))
      || !Number.isSafeInteger(Number(options.get('--attempt')))
      || !['apply', 'verify'].includes(options.get('--identity-mode')))
    throw new Error('身份生产者必须绑定绝对 run、计划、账本、正整数 attempt 和明确模式')
  return { runDirectory: options.get('--run-dir'), attempt: Number(options.get('--attempt')),
    plan: options.get('--identity-plan'), state: options.get('--identity-state'), mode: options.get('--identity-mode') }
}

export async function runIdentityProducer(argv, input = process.stdin) {
  const args = identityProducerArguments(argv)
  await waitForProducerStart(args, input)
  const lockOwnerToken = process.env.RYFRAME_DEVEX_IDENTITY_LOCK_OWNER
  if (typeof lockOwnerToken !== 'string' ||
      !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(lockOwnerToken))
    throw new Error('身份生产者缺少控制器绑定的运行锁owner token')
  // 内核收据持久化并收到当前 run/attempt 授权后，才加载可能执行真实 API 的身份入口。
  const { main } = await import('./devex_prepare_identities.mjs')
  await main([args.mode, '--plan', args.plan, '--state-dir', args.state, '--write'], { lockOwnerToken })
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { await runIdentityProducer(process.argv.slice(2)) }
  catch { process.stderr.write('identity_producer_failed：身份生产者未完成，请核对原阶段收据和账本。\n'); process.exitCode = 1 }
}
