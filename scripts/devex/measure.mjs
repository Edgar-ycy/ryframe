import { mkdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { boundedInteger } from './config.mjs'
import { operationCatalog } from './request.mjs'
import { observe } from './telemetry.mjs'
import { load } from './load.mjs'
import { validateSuiteSelection } from './selection.mjs'
import { homepage } from './homepage.mjs'
import { prepareImportSamples } from './import-samples.mjs'
import { verifyProvenance } from './provenance.mjs'
import { createPacing } from './pacing.mjs'

export async function measure(config, values, cache, dependencies = {}, environment = process.env) {
  const concurrency = values.suite === 'homepage' ? 0 : boundedInteger(
    Number(environment.RYFRAME_DEVEX_CONCURRENCY), 10, 100, '并发')
  const selection = validateSuiteSelection(config, values.suite, concurrency)
  const artifacts = values.output + '.artifacts'
  await mkdir(artifacts, { recursive: true })
  await writeFile(path.join(artifacts, 'identity-selection.json'), JSON.stringify(selection, null, 2) + '\n', { flag: 'wx' })
  const verifyAfter = await (dependencies.verifyProvenance ?? verifyProvenance)(config, values, artifacts)
  let result, failure, pacing
  try {
    pacing = await (dependencies.createPacing ?? createPacing)(config, { ...values, artifacts })
    await pacing.prepareSample()
    result = await measureVerified(config, values, cache, concurrency, artifacts, { ...dependencies, pacing })
  }
  catch (error) { failure = error }
  try { await pacing?.close() }
  catch (error) { failure = failure ? new AggregateError([failure, error], `${failure.message}; ${error.message}`) : error }
  try { await verifyAfter() }
  catch (error) { failure = failure ? new AggregateError([failure, error], `${failure.message}; ${error.message}`) : error }
  if (failure) throw failure
  return result
}

async function measureVerified(config, values, cache, concurrency, artifacts, dependencies) {
  const multipartSample = await (dependencies.prepareImportSamples ?? prepareImportSamples)(
    config, values.suite, concurrency, artifacts)
  const catalog = values.suite === 'homepage' ? undefined :
    await (dependencies.operationCatalog ?? operationCatalog)(values.backend)
  // 离线样本准备及完整性校验完成后才开始资源观察和业务请求计时。
  const stop = await (dependencies.observe ?? observe)(config)
  let measurement, resources
  try {
    measurement = values.suite === 'homepage'
      ? await (dependencies.homepage ?? homepage)(config, values.frontend, cache, artifacts)
      : await (dependencies.load ?? load)(config, catalog, values.suite, concurrency, artifacts, { multipartSample, pacing: dependencies.pacing })
  } finally { resources = await stop() }
  await measurement.finalize?.()
  delete measurement.finalize
  return { measurement, resources }
}
