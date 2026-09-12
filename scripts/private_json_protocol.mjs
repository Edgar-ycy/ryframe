function fail(ErrorType, message) {
  throw new ErrorType(message)
}

function skipWhitespace(value, index) {
  while (index < value.length && /\s/u.test(value[index])) index++
  return index
}

function readJsonString(value, start, ErrorType, label) {
  if (value[start] !== '"') fail(ErrorType, `${label}字段名必须是 JSON 字符串`)
  let escaped = false
  for (let index = start + 1; index < value.length; index++) {
    if (escaped) {
      escaped = false
      continue
    }
    if (value[index] === '\\') {
      escaped = true
      continue
    }
    if (value[index] !== '"') continue
    try {
      return { value: JSON.parse(value.slice(start, index + 1)), end: index + 1 }
    } catch {
      fail(ErrorType, `${label}包含无效 JSON 字符串`)
    }
  }
  fail(ErrorType, `${label}包含未结束的 JSON 字符串`)
}

function skipScalar(value, start, ErrorType, label) {
  if (value[start] === '"') return readJsonString(value, start, ErrorType, label).end
  const match = /^(?:true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/u.exec(
    value.slice(start),
  )
  if (!match) fail(ErrorType, `${label}字段只允许 JSON 标量`)
  return start + match[0].length
}

function rejectRepeatedFields(value, ErrorType, label) {
  let index = skipWhitespace(value, 0)
  if (value[index++] !== '{') fail(ErrorType, `${label}必须是 JSON 对象`)
  const fields = new Set()
  index = skipWhitespace(value, index)
  if (value[index] === '}') index++
  else {
    while (index < value.length) {
      const field = readJsonString(value, index, ErrorType, label)
      if (fields.has(field.value)) fail(ErrorType, `${label}字段重复：${field.value}`)
      fields.add(field.value)
      index = skipWhitespace(value, field.end)
      if (value[index++] !== ':') fail(ErrorType, `${label}字段缺少冒号`)
      index = skipWhitespace(value, index)
      index = skipWhitespace(value, skipScalar(value, index, ErrorType, label))
      if (value[index] === '}') {
        index++
        break
      }
      if (value[index++] !== ',') fail(ErrorType, `${label}字段缺少分隔符`)
      index = skipWhitespace(value, index)
    }
  }
  if (skipWhitespace(value, index) !== value.length) fail(ErrorType, `${label}包含多余内容`)
}

export function strictScalarJsonObject(
  source,
  ErrorType,
  label,
  maximumLength = 32 * 1024,
) {
  if (
    typeof source !== 'string' ||
    !source ||
    source.length > maximumLength ||
    /[\n\r\0]/u.test(source)
  )
    fail(ErrorType, `${label}缺失、为空、过长或包含换行/NUL`)
  rejectRepeatedFields(source, ErrorType, label)
  let value
  try {
    value = JSON.parse(source)
  } catch {
    fail(ErrorType, `${label}不是有效 JSON`)
  }
  if (!value || typeof value !== 'object' || Array.isArray(value))
    fail(ErrorType, `${label}必须是 JSON 对象`)
  return value
}
