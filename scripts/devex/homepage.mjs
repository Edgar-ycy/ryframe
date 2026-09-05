import { createRequire } from 'node:module'
import path from 'node:path'
import { appendFile, mkdir } from 'node:fs/promises'
import { setTimeout as delay } from 'node:timers/promises'
import { summarize } from './load.mjs'
import { homepageFailure, homepageVisitors } from './homepage-model.mjs'

export async function homepage(config, frontend, cache, artifacts) {
  const { identities, interval } = homepageVisitors(config)
  const require = createRequire(path.join(frontend, 'package.json'))
  const { chromium } = require('@playwright/test')
  const browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || (process.env.CI ? undefined : 'chrome') })
  const navigation = [], lcp = [], content = [], network = []
  let failed = 0, sessionFailures = 0
  const started = performance.now()
  try {
    await mkdir(artifacts, { recursive: true })
    for (let index = 0; index < config.contract.cycles; index++) {
      if (index > 0) await delay(interval)
      const identity = identities[index]
      const cycleStarted = performance.now()
      const evidence = { cycle: index, cache, client_address: identity.client_address,
        phase: 'preparing', succeeded: false, session_failure: false,
        preparation_requests: 0, measured_requests: 0, diagnostics: [] }
      const context = await browser.newContext({ baseURL: config.bindings.frontend_url, locale: 'zh-CN',
        extraHTTPHeaders: { 'X-Forwarded-For': identity.client_address },
        recordVideo: { dir: artifacts } })
      context.setDefaultTimeout(config.contract.timeout_ms)
      await context.tracing.start({ screenshots: true, snapshots: true })
      const page = await context.newPage()
      let errors = 0
      const diagnostic = (kind, status) => {
        if (evidence.phase === 'closing') return
        errors++
        evidence.diagnostics.push({ phase: evidence.phase, kind, ...(status === undefined ? {} : { status }) })
      }
      page.on('pageerror', () => diagnostic('pageerror'))
      page.on('console', (message) => { if (['warning', 'error'].includes(message.type())) diagnostic(message.type()) })
      page.on('request', () => {
        if (['preparing', 'login', 'warming'].includes(evidence.phase)) evidence.preparation_requests++
        if (evidence.phase === 'measuring') evidence.measured_requests++
      })
      page.on('requestfailed', () => diagnostic('requestfailed'))
      page.on('response', (response) => { if (response.status() >= 400) diagnostic('http', response.status()) })
      try {
        if (!process.env[identity.password_env]) throw new Error('首页客户密码环境变量未设置')
        await page.goto('/login')
        const tenant = page.getByPlaceholder('租户标识')
        if (await tenant.isVisible()) { await tenant.fill(identity.tenant_id); await tenant.press('Tab') }
        await page.getByPlaceholder('用户名').fill(identity.username)
        await page.getByPlaceholder('密码').fill(process.env[identity.password_env])
        evidence.phase = 'login'
        await page.getByRole('button', { name: '登录', exact: true }).click()
        await page.waitForURL('**/index')
        evidence.phase = 'warming'
        await page.locator('main.workspace').waitFor()
        await page.waitForLoadState('networkidle')
        evidence.preparation_duration_ms = performance.now() - cycleStarted
        if (errors) throw new Error('首页准备存在诊断错误')
        const cdp = await context.newCDPSession(page)
        await cdp.send('Network.enable')
        if (cache === 'cold') await cdp.send('Network.clearBrowserCache')
        await page.addInitScript(() => {
          window.__ryframeLcp = 0
          new PerformanceObserver((list) => {
            for (const entry of list.getEntries()) window.__ryframeLcp = entry.startTime
          }).observe({ type: 'largest-contentful-paint', buffered: true })
        })
        evidence.phase = 'measuring'
        await page.goto('/index', { waitUntil: 'domcontentloaded' })
        await page.getByText('已登录', { exact: true }).waitFor()
        const available = await page.evaluate(() => performance.now())
        await page.waitForLoadState('networkidle')
        const times = await page.evaluate(() => ({
          navigation: performance.getEntriesByType('navigation')[0].domContentLoadedEventEnd,
          lcp: window.__ryframeLcp,
        }))
        if (errors || !times.lcp) throw new Error('首页存在诊断错误或缺少真实 LCP')
        evidence.succeeded = true
        evidence.times = { ...times, content: available }
        navigation.push(times.navigation); lcp.push(times.lcp); content.push(available)
      } catch (error) {
        failed++
        Object.assign(evidence, homepageFailure(evidence.phase, error))
        if (evidence.session_failure) sessionFailures++
        await page.screenshot({ path: path.join(artifacts, `failure-${index}.png`) }).catch(() => {})
        await context.tracing.stop({ path: path.join(artifacts, `trace-${index}.zip`) })
      } finally {
        evidence.duration_ms = performance.now() - cycleStarted
        network.push(evidence.measured_requests)
        await appendFile(path.join(artifacts, 'homepage-cycles.jsonl'), JSON.stringify(evidence) + '\n')
        evidence.phase = 'closing'
        await context.close()
      }
    }
  } finally { await browser.close() }
  const elapsed = performance.now() - started
  return { scenarios: { navigation: summarize(navigation, failed, elapsed), lcp: summarize(lcp, failed, elapsed),
    content: summarize(content, failed, elapsed) }, network_requests: network.reduce((sum, value) => sum + value, 0),
    completed_cycles: navigation.length, failed_cycles: failed, session_failures: sessionFailures }
}
