import test from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { once } from 'node:events'
import { metricsHeaders, observe, resourceCollector } from './telemetry.mjs'

function service(cpu, started = 100, memory = 10) {
  return `ryframe_process_cpu_seconds_total ${cpu}\nryframe_process_start_time_seconds ${started}\nryframe_process_resident_memory_bytes ${memory}`
}
const database = (connections) => `mysql_up 1\nmysql_exporter_collector_success{collector="collect.global_status"} 1\nmysql_global_status_threads_connected ${connections}`

test('逐进程CPU增量汇总，内存与连接记录采样峰值', () => {
  const { result, collect } = resourceCollector()
  collect([service(10), service(20, 200)], [database(3)])
  collect([service(12, 100, 20), service(25, 200, 30)], [database(5)])
  collect([service(13), service(30, 200)], [database(4)])
  assert.deepEqual(result, { cpu_seconds: 13, peak_resident_memory_bytes: 50,
    peak_database_connections: 5, collector_failures: 0 })
})

test('其他进程CPU增长不能掩盖单个进程重启或CPU回退', () => {
  for (const changed of [service(1, 101), service(1), service(11, 101)]) {
    const { result, collect } = resourceCollector()
    collect([service(10), service(20, 200)], [database(3)])
    assert.throws(() => collect([changed, service(1000, 200)], [database(5)]), /服务重启或单进程/)
    assert.equal(result.cpu_seconds, 0)
    assert.equal(result.peak_database_connections, 3)
  }
})

test('缺失启动时间、数据库或服务集合改变均拒绝计入有效样本', () => {
  const { collect } = resourceCollector()
  assert.throws(() => collect(['ryframe_process_cpu_seconds_total 1'], [database(1)]), /process_start_time/)
  assert.throws(() => collect([service(1)], []), /缺少服务或数据库/)
  collect([service(1)], [database(1)])
  assert.throws(() => collect([service(2), service(2, 200)], [database(1)]), /服务重启或单进程/)
  assert.throws(() => collect([service(2)], ['missing 0']), /mysql_up/)
})

test('数据库失败、残留连接值、重复与无效计数均不更新资源结果', () => {
  const valid = database(5)
  const invalid = [
    valid.replace('mysql_up 1', 'mysql_up 0'),
    valid.replace('global_status"} 1', 'global_status"} 0'),
    valid.replace('mysql_up 1\n', ''),
    valid.replace('collect.global_status', 'collect.info_schema.tables'),
    valid + '\nmysql_up 1',
    valid + '\nmysql_global_status_threads_connected 10',
    database(1.5), database(-1), database(NaN),
  ]
  for (const source of invalid) {
    const { collect, result } = resourceCollector()
    collect([service(10)], [database(3)])
    const before = { ...result }
    assert.throws(() => collect([service(20, 100, 20)], [source]), /数据库|mysql_/)
    assert.deepEqual(result, before)
  }
})

test('指标密码仅从明确环境变量读取，配置错误失败关闭', () => {
  assert.deepEqual(metricsHeaders({}, {}), {})
  assert.deepEqual(metricsHeaders({ metrics_token_env: 'TEST_TOKEN' }, { TEST_TOKEN: 'fixture' }),
    { Authorization: 'Bearer fixture' })
  for (const [name, environment] of [['token', {}], ['TEST_TOKEN', {}],
    ['TEST_TOKEN', { TEST_TOKEN: 'line\nbreak' }]]) {
    assert.throws(() => metricsHeaders({ metrics_token_env: name }, environment), /指标认证/)
  }
})

test('采集真实HTTP前缀指标，Bearer只发给已登记API和Worker', async () => {
  const calls = []
  const server = createServer((request, response) => {
    calls.push({ url: request.url, authorization: request.headers.authorization })
    if (request.url === '/database') response.end(database(4))
    else if (request.headers.authorization === 'Bearer telemetry-test-fixture') response.end(service(12))
    else response.writeHead(401).end('')
  })
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  const previous = process.env.RYFRAME_TEST_METRICS_TOKEN
  process.env.RYFRAME_TEST_METRICS_TOKEN = 'telemetry-test-fixture'
  try {
    const base = `http://127.0.0.1:${server.address().port}`
    const stop = await observe({ bindings: { metrics_token_env: 'RYFRAME_TEST_METRICS_TOKEN',
      metrics_urls: { api: `${base}/api`, worker: `${base}/worker` }, database_metrics_urls: [`${base}/database`] } })
    const result = await stop()
    assert.equal(result.collector_failures, 0)
    assert.equal(result.peak_resident_memory_bytes, 20)
    assert.equal(result.peak_database_connections, 4)
    assert.equal(calls.length, 6)
    assert.ok(calls.filter((call) => call.url === '/database').every((call) => call.authorization === undefined))
    assert.ok(calls.filter((call) => call.url !== '/database').every((call) => call.authorization === 'Bearer telemetry-test-fixture'))
  } finally {
    if (previous === undefined) delete process.env.RYFRAME_TEST_METRICS_TOKEN
    else process.env.RYFRAME_TEST_METRICS_TOKEN = previous
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
  }
})
