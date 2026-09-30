import { createHash } from 'node:crypto'
import { lstat, readFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import readline from 'node:readline'
import { verifyCloneExisting } from './restore_reference_existing.mjs'

export function cloneExistingArguments(argv) {
  const names = ['--plan', '--dataset', '--target-binding', '--backend-dir', '--write', '--run-dir', '--attempt']
  const args = new Map()
  for (let index = 0; index < argv.length; index++) {
    const name = argv[index]
    if (!names.includes(name) || args.has(name)) throw new Error('复制数据检查参数未知或重复')
    const value = name === '--write' ? true : argv[++index]
    if (!value || (typeof value === 'string' && value.startsWith('--')))
      throw new Error('复制数据检查参数缺少值')
    args.set(name, value)
  }
  if (names.some((name) => !args.has(name)))
    throw new Error('必须明确原计划、数据收据、目标绑定、后端目录、run、attempt 和 --write')
  if (!path.isAbsolute(args.get('--run-dir')) || !/^[1-9][0-9]*$/.test(args.get('--attempt'))
      || !Number.isSafeInteger(Number(args.get('--attempt'))))
    throw new Error('Node 生产者必须绑定明确绝对 run 路径和正整数 attempt')
  return {
    planPath: path.resolve(args.get('--plan')),
    datasetPath: path.resolve(args.get('--dataset')),
    bindingPath: path.resolve(args.get('--target-binding')),
    backend: path.resolve(args.get('--backend-dir')),
    runDirectory: args.get('--run-dir'),
    attempt: Number(args.get('--attempt')),
  }
}

export async function waitForProducerStart({ runDirectory, attempt }, stream = process.stdin) {
  const input = readline.createInterface({ input: stream, crlfDelay: Infinity })
  try {
    for await (const line of input) {
      if (Buffer.byteLength(line) > 4096) throw new Error('生产者启动授权过大')
      const value = JSON.parse(line)
      if (Object.keys(value).sort().join(',') !== 'attempt,operation,run_dir'
          || value.operation !== 'start' || value.run_dir !== runDirectory || value.attempt !== attempt)
        throw new Error('生产者启动授权不属于当前 run 和 attempt')
      return
    }
    throw new Error('生产者尚未取得持久身份授权，禁止请求服务')
  } finally { input.close() }
}

async function readInput(filename) {
  const metadata = await lstat(filename)
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size > 16 * 1024 * 1024)
    throw new Error('复制数据检查输入必须为有界普通文件')
  const bytes = await readFile(filename)
  if (bytes.length !== metadata.size) throw new Error('复制数据检查输入在读取期间发生变化')
  return bytes
}

export async function verifyCloneFiles({ planPath, datasetPath, bindingPath, backend }) {
  const paths = {
    plan_sha256: planPath,
    dataset_sha256: datasetPath,
    target_binding_sha256: bindingPath,
  }
  const inputs = {}
  for (const [key, filename] of Object.entries(paths)) inputs[key] = await readInput(filename)
  const parse = (key) => JSON.parse(inputs[key].toString('utf8'))
  // 统一 Python 阶段负责持锁验证权威复制收据与目标进程；此内部入口只执行业务读取。
  const result = await verifyCloneExisting(
    parse('plan_sha256'),
    backend,
    parse('dataset_sha256'),
    parse('target_binding_sha256'),
  )
  for (const [key, filename] of Object.entries(paths))
    if (!inputs[key].equals(await readInput(filename)))
      throw new Error('原数据来源或复制目标绑定文件在业务验证期间发生变化')
  result.input_files = Object.fromEntries(
    Object.entries(inputs).map(([key, bytes]) => [
      key,
      createHash('sha256').update(bytes).digest('hex'),
    ]),
  )
  return result
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = cloneExistingArguments(process.argv.slice(2))
  await waitForProducerStart(args)
  const result = await verifyCloneFiles(args)
  process.stdout.write(JSON.stringify(result) + '\n')
}
