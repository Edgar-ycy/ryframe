import { spawn } from 'node:child_process'
import { createHash, randomBytes } from 'node:crypto'
import { lstat, mkdir, readFile, realpath, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { hash } from './config.mjs'
import { pythonInvocation } from './python_process.mjs'

export const importSampleModel = Object.freeze({ version: 1, rows_per_file: 1,
  username: 'dv{namespace}{worker:02x}{cycle:04x}', nickname: '性能导入',
  email: '{username}@example.test', phone: '', department: 'current_template_sheet2_A2' })
const digest = (content) => createHash('sha256').update(content).digest('hex')
const producer = fileURLToPath(new URL('../python/user_import_fixture.py', import.meta.url))

export function importSpecification(config) {
  const contract = config.contract.import_samples
  const binding = config.bindings.import_samples
  if (!contract || Object.keys(contract).sort().join() !== 'model_sha256,template_sha256' ||
      contract.model_sha256 !== hash(importSampleModel) || !/^[a-f0-9]{64}$/.test(contract.template_sha256) ||
      !binding || Object.keys(binding).sort().join() !== 'python,template' ||
      !path.isAbsolute(binding.python) || !path.isAbsolute(binding.template)) {
    throw new Error('导入性能样本必须显式绑定当前模板、固定全有效模型和 Python')
  }
  return { ...binding, ...contract }
}

function python(executable, request) {
  return new Promise((resolve, reject) => {
    const invocation = pythonInvocation({ script: producer, arguments: ['performance'] })
    const child = spawn(executable, invocation.argv, {
      env: invocation.env, windowsHide: true, stdio: ['pipe', 'pipe', 'ignore'],
    })
    const chunks = []
    let bytes = 0
    let failure
    const timer = setTimeout(() => { failure = '导入样本准备超时'; child.kill() }, 300000)
    child.on('error', () => { clearTimeout(timer); reject(new Error('导入样本准备器启动失败')) })
    child.stdout.on('data', (chunk) => {
      bytes += chunk.length
      if (bytes > 4 * 1024 * 1024) { failure = '导入样本清单超出限额'; child.kill() }
      else chunks.push(chunk)
    })
    child.stdin.on('error', () => { failure ??= '导入样本准备参数传输失败' })
    child.on('close', (code) => {
      clearTimeout(timer)
      try {
        const result = JSON.parse(Buffer.concat(chunks).toString('utf8'))
        if (failure || code !== 0 || result.ok !== true) throw new Error()
        resolve(result.manifest)
      } catch { reject(new Error(failure || '导入样本离线准备失败')) }
    })
    child.stdin.end(JSON.stringify(request))
  })
}

export async function sampleReader(directory, manifest, expected) {
  if (manifest.format_version !== 1 || manifest.namespace !== expected.namespace ||
      manifest.template_sha256 !== expected.template_sha256 || manifest.model_sha256 !== expected.model_sha256 ||
      hash(manifest.model) !== expected.model_sha256 || manifest.concurrency !== expected.concurrency ||
      manifest.cycles !== expected.cycles || !Array.isArray(manifest.files) ||
      manifest.files.length !== expected.concurrency * expected.cycles || (await lstat(directory)).isSymbolicLink()) {
    throw new Error('导入样本清单与本次准备请求不一致')
  }
  const root = await realpath(directory)
  const files = new Map()
  for (const file of manifest.files) {
    const match = /^(0|[1-9][0-9]*):(0|[1-9][0-9]*)$/.exec(file.sample)
    const worker = Number(match?.[1]), cycle = Number(match?.[2])
    const username = `dv${expected.namespace}${worker.toString(16).padStart(2, '0')}${cycle.toString(16).padStart(4, '0')}`
    if (!match || worker >= expected.concurrency || cycle >= expected.cycles || files.has(file.sample) ||
        file.filename !== `${worker}-${cycle}.xlsx` || file.username !== username || !/^[a-f0-9]{64}$/.test(file.sha256)) {
      throw new Error('导入样本清单存在未知、重复或越界样本')
    }
    files.set(file.sample, Object.freeze({ filename: file.filename, sha256: file.sha256 }))
  }
  return async (sample) => {
    const file = files.get(sample)
    if (!file) throw new Error('未登记当前测量 sample 中的导入文件')
    const filename = path.join(root, file.filename)
    const info = await lstat(filename)
    if (!info.isFile() || info.isSymbolicLink() || info.size > 16 * 1024 * 1024 ||
        path.relative(root, await realpath(filename)) !== file.filename) {
      throw new Error('导入样本文件越界或不是受控普通文件')
    }
    const content = await readFile(filename)
    if (digest(content) !== file.sha256) throw new Error('导入样本文件SHA已变化')
    return content
  }
}

export async function prepareImportSamples(config, suite, concurrency, artifacts, run = python) {
  if (!config.contract.workloads?.[suite]?.some((workflow) => workflow.job?.kind === 'import')) return undefined
  const specification = importSpecification(config)
  if (!Number.isSafeInteger(concurrency) || concurrency < 1 || concurrency > 100 ||
      !Number.isSafeInteger(config.contract.cycles) || config.contract.cycles < 1 ||
      config.contract.cycles > 10000 || concurrency * config.contract.cycles > 10000) {
    throw new Error('导入样本维度无效或单次准备超过10000个文件')
  }
  await mkdir(artifacts, { recursive: true })
  const namespace = randomBytes(10).toString('hex')
  const directory = path.resolve(artifacts, `import-${namespace}`)
  const { python: executable, ...input } = specification
  const request = { ...input, namespace, directory, concurrency, cycles: config.contract.cycles }
  const started = performance.now()
  let manifest, failure
  try {
    manifest = await run(executable, request)
    const reader = await sampleReader(directory, manifest, request)
    for (const file of manifest.files) await reader(file.sample)
    return reader
  } catch (error) { failure = 'import_sample_preparation_failed'; throw error }
  finally {
    await writeFile(path.join(artifacts, 'import-preparation.json'), JSON.stringify({
      format_version: 1, namespace, directory, template_sha256: input.template_sha256,
      model_sha256: input.model_sha256, duration_ms: performance.now() - started,
      success: !failure, failure, manifest,
    }, null, 2) + '\n', { flag: 'wx' })
  }
}
