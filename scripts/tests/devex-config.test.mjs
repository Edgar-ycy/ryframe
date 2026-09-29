import test from 'node:test'
import assert from 'node:assert/strict'
import { validateMetricsBindings } from '../devex/config.mjs'

const bindings = () => ({ metrics_urls: { api: 'http://127.0.0.1:18080/metrics',
  worker: 'http://127.0.0.1:19091/metrics' }, database_metrics_urls: ['http://127.0.0.1:19104/metrics'] })

test('进程指标严格仅API/Worker，数据库端点必须非空并且唯一', () => {
  assert.doesNotThrow(() => validateMetricsBindings(bindings()))
  for (const change of [
    (value) => { delete value.metrics_urls },
    (value) => { delete value.metrics_urls.worker },
    (value) => { value.metrics_urls.scheduler = 'http://127.0.0.1:19999/metrics' },
    (value) => { value.metrics_urls = [] },
    (value) => { value.database_metrics_urls = [] },
    (value) => { value.database_metrics_urls.push(value.database_metrics_urls[0]) },
    (value) => { value.metrics_urls.api = 'https://secret:password@example.test/metrics' },
  ]) {
    const value = bindings()
    change(value)
    assert.throws(() => validateMetricsBindings(value))
  }
})

test('URL规范化后重复也拒绝，防止默认端口和路径变体重复计数', () => {
  for (const [first, second] of [
    ['http://LOCALHOST:80/metrics', 'http://localhost/metrics'],
    ['https://localhost:443/metrics', 'https://localhost/metrics'],
    ['http://127.0.0.1/a/../metrics', 'http://127.0.0.1/metrics'],
    ['http://127.0.0.1', 'http://127.0.0.1/'],
  ]) {
    const value = bindings()
    value.metrics_urls = { api: first, worker: second }
    assert.throws(() => validateMetricsBindings(value), /规范化后不能重复/)
    value.metrics_urls = bindings().metrics_urls
    value.database_metrics_urls = [first, second]
    assert.throws(() => validateMetricsBindings(value), /规范化后不能重复/)
  }
})
