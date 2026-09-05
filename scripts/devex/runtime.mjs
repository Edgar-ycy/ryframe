import { readFile, writeFile, mkdir } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { configuration } from './config.mjs'
import { measure } from './measure.mjs'

class ParameterError extends Error {}

const DRIVER_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')

function runtimeArguments(args) {
  let values
  try {
    ;({ values } = parseArgs({
      args,
      options: {
        suite: { type: 'string' },
        backend: { type: 'string' },
        frontend: { type: 'string' },
        output: { type: 'string' },
      },
      strict: true,
      allowPositionals: false,
    }))
  } catch (error) {
    throw new ParameterError(error.message)
  }
  if (!['api', 'jobs', 'tenants', 'homepage'].includes(values.suite)) {
    throw new ParameterError('未知运行时场景')
  }
  for (const name of ['backend', 'frontend', 'output']) {
    if (typeof values[name] !== 'string' || !path.isAbsolute(values[name])) {
      throw new ParameterError(`--${name} 必须是绝对路径`)
    }
  }
  return values
}

export async function runRuntimeDriver(
  args = process.argv.slice(2),
  environment = process.env,
  dependencies = {},
) {
  const report = dependencies.report ?? ((message) => console.error(message))
  let values
  try {
    values = runtimeArguments(args)
  } catch (error) {
    report(`运行时测量参数无效：${error.message}`)
    return 2
  }

  const read = dependencies.readFile ?? readFile
  const configure = dependencies.configuration ?? configuration
  const execute = dependencies.measure ?? measure
  const makeDirectory = dependencies.mkdir ?? mkdir
  const write = dependencies.writeFile ?? writeFile
  try {
    const configPath = path.join(values.backend, '.local-tests/devex/runtime.json')
    const before = await read(configPath)
    const config = await configure(values.backend, values.suite, environment)
    const cache = environment.RYFRAME_DEVEX_CACHE
    if (!['cold', 'warm'].includes(cache)) throw new Error('必须明确冷暖状态')
    const driverFingerprint = environment.RYFRAME_DEVEX_DRIVER_FINGERPRINT
    if (!/^sha256:[a-f0-9]{64}$/.test(driverFingerprint ?? '')) {
      throw new Error('必须绑定实际运行 driver 的完整源码指纹')
    }
    const runtimeValues = {
      ...values,
      driver: DRIVER_ROOT,
      driver_fingerprint: driverFingerprint,
    }
    const { measurement, resources } = await execute(
      config,
      runtimeValues,
      cache,
      dependencies.measureDependencies,
      environment,
    )
    const after = await read(configPath)
    if (!before.equals(after)) throw new Error('运行期间测量配置发生变化')
    const result = {
      input_sha256: environment.RYFRAME_DEVEX_RUNTIME_INPUT_SHA256,
      cache_state: cache,
      ...measurement,
      ...resources,
    }
    await makeDirectory(path.dirname(values.output), { recursive: true })
    await write(values.output, JSON.stringify(result, null, 2) + '\n', { flag: 'wx' })
    return resources.collector_failures ||
      measurement.failed_cycles ||
      measurement.session_failures ||
      Object.values(measurement.scenarios).some((value) => value.failed || !value.completed)
      ? 1
      : 0
  } catch (error) {
    report(`运行时测量未通过：${error.message}`)
    return 1
  }
}

const invokedPath = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : undefined
if (invokedPath === import.meta.url) process.exitCode = await runRuntimeDriver()
