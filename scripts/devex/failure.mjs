const categories = new Set([
  'http_status',
  'business_response',
  'invalid_response',
  'poll_failed',
  'poll_timeout',
])

export class RequestFailure extends Error {
  constructor(category, operation, status) {
    super(
      status
        ? `${operation}: HTTP ${status}`
        : `${operation}: ${category.startsWith('poll_') ? '后台状态失败或超时' : category}`,
    )
    if (
      !categories.has(category) ||
      typeof operation !== 'string' ||
      !/^[a-z][a-z0-9_]{0,127}$/.test(operation)
    ) {
      throw new Error('请求失败分类无效')
    }
    this.category = category
    this.operation = operation
    if (Number.isInteger(status) && status >= 400 && status <= 599) this.status = status
  }
}

export function failureCategory(error) {
  if (error instanceof RequestFailure) {
    return {
      category: error.category,
      operation: error.operation,
      ...(error.status ? { status: error.status } : {}),
    }
  }
  if (error?.name === 'TimeoutError') return { category: 'request_timeout' }
  if (error?.name === 'AbortError') return { category: 'request_cancelled' }
  return { category: 'request_failed' }
}
