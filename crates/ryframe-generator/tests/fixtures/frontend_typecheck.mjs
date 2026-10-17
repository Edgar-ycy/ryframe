import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import vm from 'node:vm'
import { createRequire } from 'node:module'

const [frontend, root] = process.argv.slice(2)
const require = createRequire(path.join(frontend, 'package.json'))
const ts = require('typescript')
const resource = JSON.parse(fs.readFileSync(path.join(root, 'resource.json'), 'utf8'))
const { fields, pascal_name: pascal, name, api } = resource
const generated = `src/generated/resources/${name}`
const hasDateColumn = fields.some(field => field.usage.list && field.value_type === 'date_time' && !Object.keys(field.enum_values).length)
const write = (relative, content) => {
  const file = path.join(root, relative)
  fs.mkdirSync(path.dirname(file), { recursive: true })
  fs.writeFileSync(file, content)
}
const copy = relative => write(`src/${relative}`, fs.readFileSync(path.join(frontend, 'src', relative), 'utf8'))
// 直接使用当前前端的公开类型和资源构造器，避免测试替身放宽约束。
for (const file of ['components/business/flat-crud/model.ts', 'components/business/flat-crud/resource.ts', 'shared/http/types.ts', 'api/contract.ts']) copy(file)
write('src/components/business/flat-crud/index.ts', "export * from './model'\nexport * from './resource'\n")
const properties = (predicate, optional) => fields.filter(predicate).map(field => {
  const suffix = optional(field) ? '?' : ''
  return `  ${field.name}${suffix}: ${field.typescript_type}`
}).join('\n')
const record = properties(field => field.usage.read || field.usage.list, () => false)
const query = properties(field => field.usage.filter, () => true)
const create = properties(field => field.usage.create, field => field.usage.create_optional)
const update = properties(field => field.usage.update, field => field.usage.update_optional)
const operations = api.operations
write('src/api/generated/schema.ts', `
export interface RecordVo {\n${record}\n}
export interface Query { page?: number; page_size?: number;\n${query}\n}
export interface Create {\n${create}\n}
export interface Update {\n${update}\n}
export interface components { schemas: { ${pascal}Vo: RecordVo } }
export interface operations {
  ${operations.list}: { parameters: { query?: Query } }
  ${operations.create}: { requestBody: { content: { 'application/json': Create } } }
  ${operations.update}: { requestBody: { content: { 'application/json': Update } } }
}
`)
// 仅传输端口使用确定的离线实现；响应形状继续复用真实 HTTP 类型。
write('src/api/generated/operations/system.ts', `
import type { RecordVo, Query, Create, Update } from '../schema'
import type { ApiResponse, PageResponse } from '@/shared/http/types'
export async function ${operations.list}(_input: { params: Query; signal?: AbortSignal }): Promise<ApiResponse<PageResponse<RecordVo>>> { throw new Error('离线端口') }
export async function ${operations.read}(_input: { path: { id: string }; signal?: AbortSignal }): Promise<ApiResponse<RecordVo>> { throw new Error('离线端口') }
export async function ${operations.create}(_input: { data: Create }): Promise<void> {}
export async function ${operations.update}(_input: { path: { id: string }; data: Update }): Promise<void> {}
export async function ${operations.delete}(_input: { path: { id: string } }): Promise<void> {}
`)
const precisionChecks = fields.some(field => field.name === 'value_2') ? `
const decimal: string = form.value_0
const signed: string = form.value_1
const unsigned: string = form.value_2
// @ts-expect-error Decimal 必须以字符串传输。
form.value_0 = 0.1
// @ts-expect-error i64 必须以字符串传输。
form.value_1 = 9223372036854775807
// @ts-expect-error u64 必须以字符串传输。
form.value_2 = 18446744073709551615
export { decimal, signed, unsigned }
` : ''
write('contract-check.ts', `
import { create${pascal}Presentation, type ${pascal}Form } from './${generated}/fields'
const presentation = create${pascal}Presentation((zh, _en) => zh${hasDateColumn ? ", value => value" : ""})
const form: ${pascal}Form = presentation.resource.emptyForm()
export { form }
${precisionChecks}
`)
const compilerOptions = {
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.CommonJS,
  moduleResolution: ts.ModuleResolutionKind.Node10,
  strict: true,
  noUnusedLocals: true,
  noUnusedParameters: true,
  noEmit: true,
  skipLibCheck: true,
  types: [],
  baseUrl: root,
  paths: { '@/*': ['src/*'] },
  ignoreDeprecations: '6.0',
}
const program = ts.createProgram([path.join(root, 'contract-check.ts')], compilerOptions)
const diagnostics = ts.getPreEmitDiagnostics(program)
if (diagnostics.length) {
  throw new Error(ts.formatDiagnosticsWithColorAndContext(diagnostics, {
    getCurrentDirectory: () => root,
    getCanonicalFileName: file => file,
    getNewLine: () => '\n',
  }))
}
// 执行相同 TS 源码，核验聚合没有遗漏、重复、乱序或改变值类型。
const modules = new Map()
function load(file) {
  if (modules.has(file)) return modules.get(file).exports
  const module = { exports: {} }
  modules.set(file, module)
  const source = fs.readFileSync(file, 'utf8')
  const code = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText
  const localRequire = specifier => {
    const target = specifier.startsWith('@/')
      ? path.join(root, 'src', specifier.slice(2))
      : path.resolve(path.dirname(file), specifier)
    return load(fs.existsSync(`${target}.ts`) ? `${target}.ts` : path.join(target, 'index.ts'))
  }
  vm.runInThisContext(`(function(require, module, exports) { ${code}\n})`, { filename: file })(localRequire, module, module.exports)
  return module.exports
}
const factory = load(path.join(root, generated, 'fields.ts'))[`create${pascal}Presentation`]
const zh = factory((zh, _en) => zh, value => `日期:${value}`)
const en = factory((_zh, en) => en, value => `Date:${value}`)
const formFields = fields.filter(field => field.usage.create || field.usage.update)
const queryFields = fields.filter(field => field.usage.filter && field.widget !== 'hidden')
assert.deepEqual(Array.from(zh.columns, field => field.key), fields.filter(field => field.usage.list).map(field => field.name))
assert.deepEqual(Array.from(zh.formFields, field => field.key), formFields.map(field => field.name))
assert.deepEqual(Array.from(zh.queryFields, field => field.key), queryFields.map(field => field.name))
const form = zh.resource.emptyForm()
assert.deepEqual(Object.keys(form), formFields.map(field => field.name))
assert.deepEqual(Object.keys(zh.resource.createInput(form)), fields.filter(field => field.usage.create).map(field => field.name))
assert.deepEqual(Object.keys(zh.resource.updateInput(form)), fields.filter(field => field.usage.update).map(field => field.name))
const recordValues = Object.fromEntries(fields.map(field => [field.name, field.nullable ? null : field.wire_type === 'string' ? '12345678901234567890.125000' : 1]))
const editForm = zh.resource.editForm(recordValues)
for (const field of formFields) assert.equal(editForm[field.name], recordValues[field.name])
for (const field of queryFields.filter(field => field.value_type === 'string')) assert.equal(zh.resource.initialQuery()[field.name], '')
for (const field of fields) {
  const column = zh.columns.find(column => column.key === field.name)
  const english = en.columns.find(column => column.key === field.name)
  if (column) {
    assert.equal(column.label, field.labels.zh_cn)
    assert.equal(english.label, field.labels.en)
    if (Object.keys(field.enum_values).length) {
      const entries = Object.entries(field.enum_values).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)
      assert.deepEqual(Array.from(column.options, option => option.label), entries.map(([, label]) => label.zh_cn))
      const enumValue = key => field.wire_type === 'string' ? key : field.value_type === 'bool' ? key === 'true' : Number(key)
      assert.deepEqual(Array.from(column.options, option => option.value), entries.map(([key]) => enumValue(key)))
      assert.equal(column.positiveValue, enumValue(entries.at(-1)[0]))
    } else if (field.value_type === 'date_time') assert.equal(column.format('2026'), '日期:2026')
  }
  if (!(field.usage.create || field.usage.update)) continue
  const control = zh.formFields.find(control => control.key === field.name)
  const englishControl = en.formFields.find(control => control.key === field.name)
  assert.equal(control.label, field.labels.zh_cn)
  assert.equal(englishControl.label, field.labels.en)
  if (field.widget === 'text' || field.widget === 'textarea') {
    assert.equal(control.placeholder, `请输入${field.labels.zh_cn}`)
    assert.equal(englishControl.placeholder, `Enter ${field.labels.en}`)
  }
  assert.equal(control.disabledOnEdit === true, field.usage.create && !field.usage.update)
  assert.equal(control.editOnly === true, field.usage.update && !field.usage.create)
  if (['decimal', 'i64', 'u64'].includes(field.value_type) && field.wire_type === 'string') {
    assert.equal(form[field.name], '')
    const precise = { ...form, [field.name]: '18446744073709551615.125000' }
    for (const action of ['create', 'update']) {
      if (!field.usage[action]) continue
      const convert = zh.resource[`${action}Input`]
      assert.equal(convert(precise)[field.name], precise[field.name])
      assert.equal(convert(form)[field.name], field.nullable ? null : field.usage[`${action}_optional`] ? undefined : '')
    }
  }
}
for (const field of queryFields) {
  const zhQuery = zh.queryFields.find(control => control.key === field.name)
  const enQuery = en.queryFields.find(control => control.key === field.name)
  assert.equal(zhQuery.label, field.labels.zh_cn)
  assert.equal(enQuery.label, field.labels.en)
  assert.equal(zhQuery.placeholder, `请输入或选择${field.labels.zh_cn}`)
  assert.equal(enQuery.placeholder, `Enter or select ${field.labels.en}`)
}
const importLikeField = fields.find(field => field.labels.zh_cn === "hello from '../path")
if (importLikeField) {
  const column = zh.columns.find(column => column.key === importLikeField.name)
  assert.deepEqual(Buffer.from(column.label, 'utf8'), Buffer.from(importLikeField.labels.zh_cn, 'utf8'))
}
assert.deepEqual({ ...zh.permissions }, { list: resource.access.permissions.list, create: resource.access.permissions.create, update: resource.access.permissions.update, remove: resource.access.permissions.delete })
assert.equal(zh.labels.title, resource.labels.zh_cn)
assert.equal(en.labels.title, resource.labels.en)
assert.equal(zh.resource.recordId(recordValues), String(recordValues.id))
const visibleString = field => field.value_type === 'string' && (field.usage.read || field.usage.list)
const label = fields.find(field => field.name === 'name' && visibleString(field)) ?? fields.find(visibleString)
assert.ok(zh.resource.messages.deleteConfirm(recordValues).includes(String(recordValues[label?.name ?? 'id'])))
console.log(`TypeScript 严格类型及运行时验证通过：${formFields.length} 个表单字段`)
