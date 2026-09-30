import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, rm } from 'node:fs/promises'
import path from 'node:path'
import { collectJob, jobSelection } from './job-timing.mjs'
import { completeMeasurements } from './load.mjs'

const workflow = {
  name: 'export',
  job: { kind: 'export', id: '${export_id}', job_type: 'system.export.execute' },
}
const variables = { tenant_id: 'tenant-a', export_id: '101', worker: 0, cycle: 0 }
const config = {
  contract: { timeout_ms: 1000 },
  bindings: {
    scope_id: 'devex-test-scope',
    job_timings: {
      python: path.resolve('python-fixture'),
      control: { database: 'test_control' },
      server_uuid: 'fixture',
      mysql_client: path.resolve('mysql-fixture'),
    },
  },
}
const proof = {
  format_version: 1,
  scope_id: 'devex-test-scope',
  kind: 'export',
  business_id: '101',
  valid: true,
  queue_ms: 12,
  execution_ms: 25,
  attempt_count: 2,
  unsuccessful_attempts: 1,
  attempts: [
    { sequence: 1, outcome: 'failed' },
    { sequence: 2, outcome: 'succeeded' },
  ],
}

async function artifacts(t) {
  const root = path.resolve('.local-tests')
  await mkdir(root, { recursive: true })
  const directory = await mkdtemp(path.join(root, 'job-timing-test-'))
  t.after(async () => {
    assert.equal(path.dirname(directory), root)
    await rm(directory, { recursive: true })
  })
  return directory
}

test('任务采集使用保存的精确业务ID且保留所有重试事实', async (t) => {
  const directory = await artifacts(t)
  const result = await collectJob(
    config,
    workflow,
    variables,
    directory,
    async (binding, request) => {
      assert.equal(binding.python, config.bindings.job_timings.python)
      assert.deepEqual(request.selection, {
        kind: 'export',
        id: '101',
        tenant_id: 'tenant-a',
        job_type: 'system.export.execute',
      })
      assert.equal(request.binding.control.database, 'test_control')
      return proof
    },
  )
  assert.deepEqual(result, [12, 25])
  const saved = JSON.parse(await readFile(path.join(directory, 'job-attempts.jsonl'), 'utf8'))
  assert.equal(saved.attempts.length, 2)
  assert.equal(saved.unsuccessful_attempts, 1)
})

test('失败取消和来源不符的采集不能进入成功耗时统计', async (t) => {
  const directory = await artifacts(t)
  for (const value of [
    { ...proof, valid: false, reason: 'job_cancelled' },
    { ...proof, scope_id: 'other-scope' },
    { ...proof, execution_ms: Number.NaN },
  ]) {
    await assert.rejects(
      collectJob(config, workflow, variables, directory, async () => value),
      /任务时间证据无效/,
    )
  }
  const saved = (await readFile(path.join(directory, 'job-attempts.jsonl'), 'utf8'))
    .trim()
    .split('\n')
    .map(JSON.parse)
  assert.equal(saved.length, 3)
  assert.ok(saved.every((row) => row.valid === false))
  assert.equal(saved[0].reason, 'job_cancelled')
})

test('采集故障保留分类而不泄漏子进程错误', async (t) => {
  const directory = await artifacts(t)
  await assert.rejects(
    collectJob(config, workflow, variables, directory, async () => {
      throw new Error('secret-fixture-value')
    }),
    /collector_execution_failed/,
  )
  const saved = await readFile(path.join(directory, 'job-attempts.jsonl'), 'utf8')
  assert.ok(!saved.includes('secret-fixture-value'))
})

test('缺失ID、注入和超出Snowflake范围的绑定被拒绝', () => {
  for (const id of ['1 OR 1=1', '0', '9223372036854775808', undefined]) {
    assert.throws(() => jobSelection(workflow, { ...variables, export_id: id }))
  }
})

test('事后证据缺失使已完成业务周期失败并保留原始耗时', async (t) => {
  const directory = await artifacts(t)
  const records = [
    { variables: { ...variables, cycle: 0 }, elapsed: 10, succeeded: true, proof: false },
    { variables: { ...variables, cycle: 1 }, elapsed: 99, succeeded: true, proof: false },
  ]
  const original = []
  const scenarios = {}
  await completeMeasurements(
    [{ workflow, records, failed: 0, elapsed: 1000 }],
    scenarios,
    config,
    directory,
    async (_config, _workflow, input) => {
      original.push([input.business_duration_ms, input.business_succeeded])
      if (input.cycle === 1) throw new Error('missing attempt')
      return [12, 25]
    },
  )
  assert.deepEqual(original, [
    [10, true],
    [99, true],
  ])
  assert.equal(records[1].elapsed, 99)
  assert.equal(scenarios.export.completed, 1)
  assert.equal(scenarios.export.failed, 1)
  assert.equal(scenarios.export.p95_ms, 10)
  assert.equal(scenarios.export.elapsed_ms, 1000)
  const cycles = (await readFile(path.join(directory, 'cycles.jsonl'), 'utf8'))
    .trim()
    .split('\n')
    .map(JSON.parse)
  assert.equal(cycles[1].business_duration_ms, 99)
  assert.equal(cycles[1].succeeded, true)
  assert.equal(cycles[1].proof, false)
  assert.equal(cycles[1].proof_failure.category, 'job_timing_evidence_invalid')
})
