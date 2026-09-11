import { spawn } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { hash } from './config.mjs'

const verifier = fileURLToPath(new URL('../devex_provenance.py', import.meta.url))

function requestFor(config, values) {
  const binding = config.bindings.provenance ?? {}
  const { python, ...provenance } = binding
  const { scope_id, source_fingerprints, api_url, frontend_url, metrics_urls } = config.bindings
  const roots = ['backend', 'frontend', 'driver', 'runner_frontend']
  const descriptors = ['runtime_receipt', 'target_plan', 'runtime_registration', 'runtime_launch']
  const fields = ['python', ...descriptors, 'environment_document'].sort().join()
  const descriptor = (value) =>
    value &&
    !Array.isArray(value) &&
    Object.keys(value).sort().join() === 'bytes,path,sha256' &&
    typeof value.path === 'string' &&
    path.isAbsolute(value.path) &&
    Number.isSafeInteger(value.bytes) &&
    value.bytes > 0 &&
    /^[a-f0-9]{64}$/.test(value.sha256)
  if (
    Object.keys(binding).sort().join() !== fields ||
    typeof python !== 'string' ||
    !path.isAbsolute(python) ||
    descriptors.some((name) => !descriptor(provenance[name])) ||
    typeof provenance.environment_document !== 'string' ||
    !path.isAbsolute(provenance.environment_document) ||
    roots.some((name) => typeof values[name] !== 'string' || !path.isAbsolute(values[name])) ||
    !/^sha256:[a-f0-9]{64}$/.test(values.driver_fingerprint ?? '') ||
    !/^sha256:[a-f0-9]{64}$/.test(values.runner_frontend_fingerprint ?? '') ||
    source_fingerprints?.runner_frontend !== values.runner_frontend_fingerprint
  )
    throw new Error('bindings')
  return { python, request: structuredClone({ provenance, scope_id, source_fingerprints,
    api_url, frontend_url, metrics_urls, backend: path.resolve(values.backend),
    frontend: path.resolve(values.frontend), driver: path.resolve(values.driver),
    runner_frontend: path.resolve(values.runner_frontend),
    driver_fingerprint: values.driver_fingerprint,
    runner_frontend_fingerprint: values.runner_frontend_fingerprint,
    environment_sha256: config.contract.environment_sha256 }) }
}

function python(executable, request) {
  return new Promise((resolve, reject) => {
    const child = spawn(executable, ['-B', '-X', 'utf8', verifier], {
      env: process.env, windowsHide: true, stdio: ['pipe', 'pipe', 'ignore'],
    })
    const chunks = []
    let bytes = 0, failure
    const timer = setTimeout(() => { failure = 'timeout'; child.kill() }, 180000)
    child.on('error', () => { clearTimeout(timer); reject(new Error('verifier_start')) })
    child.stdout.on('data', (chunk) => {
      bytes += chunk.length
      if (bytes > 4 * 1024 * 1024) { failure = 'receipt_size'; child.kill() }
      else chunks.push(chunk)
    })
    child.stdin.on('error', () => { failure ??= 'verifier_input' })
    child.on('close', (code) => {
      clearTimeout(timer)
      try {
        const result = JSON.parse(Buffer.concat(chunks).toString('utf8'))
        if (failure) throw new Error(failure)
        if (code !== 0 || result.ok !== true) throw new Error(result.stage || 'verifier_result')
        resolve(result.receipt)
      } catch (error) { reject(new Error(failure || error.message)) }
    })
    child.stdin.end(JSON.stringify(request))
  })
}

export async function verifyProvenance(config, values, artifacts, run = python) {
  await mkdir(artifacts, { recursive: true })
  let specification, before
  async function verify(phase) {
    const started = performance.now()
    let receipt, failure
    try {
      specification ??= requestFor(config, values)
      receipt = await run(specification.python, specification.request)
      if (receipt?.format_version !== 3 || receipt.kind !== 'devex-runtime-provenance')
        throw new Error('verifier_result')
      if (before && hash(receipt) !== hash(before)) throw new Error('sample_changed')
      return receipt
    } catch (error) {
      failure = /^[a-z_]{1,64}$/.test(error.message) ? error.message : 'verification_failed'
      throw new Error(`运行来源核验失败：${phase}/${failure}`)
    } finally {
      await writeFile(path.join(artifacts, `provenance-${phase}.json`), JSON.stringify({
        format_version: 1, phase, duration_ms: performance.now() - started,
        success: !failure, failure, receipt,
      }, null, 2) + '\n', { flag: 'wx' })
    }
  }
  before = await verify('before')
  return async () => { await verify('after') }
}
