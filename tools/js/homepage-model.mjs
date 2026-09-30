import { boundedInteger, httpUrl } from './config.mjs'
import { selectHomepageIdentities } from './selection.mjs'

export function homepageVisitors(config) {
  const cycles = boundedInteger(config.contract.cycles, 10, 10000, '首页访问周期')
  const interval = boundedInteger(config.contract.homepage?.visitor_interval_ms, 0, 60000, '首页访问间隔')
  for (const endpoint of [config.bindings.api_url, config.bindings.frontend_url]) {
    if (!['127.0.0.1', '[::1]'].includes(httpUrl(endpoint).hostname)) {
      throw new Error('固定首页客户模型只允许明确 loopback API 与前端')
    }
  }
  const identities = selectHomepageIdentities(config, cycles)
  return { identities, interval }
}

export function browserFailure(error) {
  if (error?.name === 'TimeoutError') return 'timeout'
  if (error?.name === 'TypeError') return 'network_or_binding'
  return 'browser_validation_failed'
}

export function homepageFailure(phase, error) {
  return { failure: browserFailure(error), session_failure: phase === 'login' }
}
