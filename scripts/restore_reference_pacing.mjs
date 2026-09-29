import { setTimeout as delay } from 'node:timers/promises'

export function requestPacer(interval, clock = () => performance.now(), sleep = delay) {
  if (!Number.isSafeInteger(interval) || interval < 1000 || interval > 5000) {
    throw new Error('参考数据准备必须显式设置1000到5000ms的每客户HTTP请求间隔')
  }
  let last = -Infinity
  let tail = Promise.resolve()
  return () => {
    const next = tail.then(async () => {
      const remaining = last + interval - clock()
      if (remaining > 0) await sleep(remaining)
      last = clock()
    })
    tail = next.catch(() => {})
    return next
  }
}
