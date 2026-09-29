import { createHash } from 'node:crypto'
import assert from 'node:assert/strict'
import { Session } from '../devex/request.mjs'
import { hash } from '../devex/config.mjs'

export const bytes = Buffer.from('original-owned-object')
export const sha256 = (value) => createHash('sha256').update(value).digest('hex')

export function fixture() {
  const plan = {
    source: {
      scope_id: 'original',
      api_url: 'http://127.0.0.1:3100',
      frontend_url: 'http://127.0.0.1:4100',
    },
    target: {
      scope_id: 'restored',
      api_url: 'http://127.0.0.1:3200',
      frontend_url: 'http://127.0.0.1:4200',
    },
    dataset: { request_interval_ms: 1000 },
  }
  const tenants = Array.from({ length: 11 }, (_, index) => ({
    tenant_id: index ? `original-${String(index).padStart(2, '0')}` : 'system',
    username: `owner-${index}`,
    password_env: `TENANT_${index}_PASSWORD`,
    posts: Array.from({ length: 3 }, (_, post) => ({
      id: String(index * 3 + post + 1),
      code: `code-${index}-${post}`,
      name: `岗位${index}-${post}`,
    })),
    files: Array.from({ length: 23 + (index < 3 ? 1 : 0) }, (_, file) => ({
      file_path: `tenant-${index}/existing-${file}.txt`,
      bytes: bytes.length,
      sha256: sha256(bytes),
    })),
  }))
  const dataset = {
    format_version: 1,
    plan_sha256: hash(plan),
    source_scope_id: 'original',
    tenants,
  }
  const binding = {
    format_version: 1,
    kind: 'devex-copy-business-target',
    source_plan_sha256: hash(plan),
    source_scope_id: 'original',
    target: {
      scope_id: 'seed-copy',
      api_url: 'http://127.0.0.1:3300',
      frontend_url: 'http://127.0.0.1:4300',
    },
    copy: Object.fromEntries(
      [
        'plan_sha256',
        'generation_sha256',
        'source_export_sha256',
        'fresh_target_sha256',
        'stage_receipt_sha256',
        'ledger_head_sha256',
      ].map((name, index) => [name, String(index + 1).repeat(64)]),
    ),
  }
  return { plan, dataset, binding }
}

export function transport(t, dataset, options = {}) {
  const calls = []
  t.mock.method(Session.prototype, 'login', async function () {
    calls.push({ operation: 'login', identity: this.identity, bindings: this.config.bindings })
    this.token = 'fixture-token'
    this.beforeRequest = async () => {}
    await options.onLogin?.()
  })
  t.mock.method(Session.prototype, 'request', async function (step) {
    calls.push({
      operation: step.operation,
      identity: this.identity,
      bindings: this.config.bindings,
    })
    if (step.operation === 'post_auth_logout') {
      if (options.logoutFailure) throw new Error('logout failed')
      return {}
    }
    assert.equal(step.operation, 'get_system_posts_by_id')
    const identity = dataset.tenants.find((value) => value.tenant_id === this.identity.tenant_id)
    const post = identity.posts.find((value) => value.id === step.path.id)
    assert.ok(post)
    return { data: { ...post, ...(options.wrongPost ? { code: 'wrong' } : {}) } }
  })
  t.mock.method(globalThis, 'fetch', async (url, request) => {
    calls.push({ operation: 'download', url, request })
    return new Response(options.wrongFile ? 'wrong' : bytes)
  })
  return calls
}
