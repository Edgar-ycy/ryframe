import test from 'node:test'
import assert from 'node:assert/strict'
import { browserFailure, homepageFailure, homepageVisitors } from '../devex/homepage-model.mjs'
import { identityPool } from './devex-selection-fixture.mjs'

function config() {
  const pool = identityPool(10)
  return { contract: { cycles: 10, homepage: { visitor_interval_ms: 250, identity_pool: 'visitors' },
    identity_pools: { visitors: pool.contract } }, bindings: {
    api_url: 'http://127.0.0.1:18080', frontend_url: 'http://127.0.0.1:4174',
    identity_pools: { visitors: pool.binding } } }
}

test('首页客户模型使用预登记身份与固定地址，保持同一顺序和间隔', () => {
  const input = config()
  const visitors = homepageVisitors(input)
  assert.equal(visitors.interval, 250)
  assert.deepEqual(visitors.identities, input.bindings.identity_pools.visitors.map((identity, index) => ({
    ...identity, client_address: input.contract.identity_pools.visitors.slots[index].client_address })))
  assert.deepEqual(homepageVisitors(input), visitors)
})

test('首页拒绝不完整、重复、外部地址以及未显式声明的客户节奏', () => {
  for (const change of [
    (value) => value.bindings.identity_pools.visitors.pop(),
    (value) => { value.bindings.identity_pools.visitors[1].username = 'user-1' },
    (value) => { value.contract.identity_pools.visitors.slots[1].client_address = '198.18.10.1' },
    (value) => { value.contract.identity_pools.visitors.slots[1].client_address = '203.0.113.1' },
    (value) => { value.contract.identity_pools.visitors.slots[1].client_address = '198.18.999.1' },
    (value) => { value.bindings.api_url = 'https://example.test' },
    (value) => { value.bindings.frontend_url = 'https://example.test' },
    (value) => { delete value.contract.homepage },
    (value) => { value.contract.homepage.visitor_interval_ms = -1 },
  ]) {
    const input = config()
    change(input)
    assert.throws(() => homepageVisitors(input))
  }
})

test('失败分类不保存浏览器错误中的凭据、地址或响应正文', () => {
  assert.equal(browserFailure({ name: 'TimeoutError', message: 'secret' }), 'timeout')
  assert.equal(browserFailure(new TypeError('secret')), 'network_or_binding')
  assert.equal(browserFailure(new Error('secret')), 'browser_validation_failed')
})

test('仅真实登录阶段失败归入会话，页面准备、缓存预热和测量失败分别保留阶段', () => {
  const phases = ['preparing', 'login', 'warming', 'measuring']
  const failures = phases.map((phase) => homepageFailure(phase, { name: 'TimeoutError', message: 'secret' }))
  assert.deepEqual(failures.map((result) => result.session_failure), [false, true, false, false])
  assert.equal(failures.filter((result) => result.session_failure).length, 1)
  assert.ok(failures.every((result) => result.failure === 'timeout'))
  assert.deepEqual(homepageFailure('login', new Error('private-response')), {
    failure: 'browser_validation_failed', session_failure: true,
  })
  assert.deepEqual(homepageFailure('warming', new TypeError('private-address')), {
    failure: 'network_or_binding', session_failure: false,
  })
  assert.ok(!JSON.stringify(failures).includes('secret'))
})
