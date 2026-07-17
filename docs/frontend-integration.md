# 前端集成指南

本文档面向独立 Git 仓库 `ryframe-vue3`，约定后端接口响应、认证、动态菜单、分页、上传下载和监控接口的使用方式。两个仓库分别构建和发布，不共享源码目录；接口字段只从 OpenAPI 生成，本文档不维护第二份完整 DTO。

## 契约同步

后端仓库将规范快照提交到 `openapi/openapi.json`，CI 会重新运行导出器并检查差异。接口变更后，在前端仓库同步本地后端快照：

```powershell
Set-Location ryframe-vue3
$env:RYFRAME_OPENAPI_SOURCE='..\openapi\openapi.json'
pnpm api:sync
pnpm api:check
```

`src/api/generated/schema.ts` 由 `openapi-typescript` 生成，`src/shared/security/passwordPolicy.generated.json` 由同步脚本从 `x-ryframe-password-policy` 生成，两者都禁止手工修改。业务 API 模块通过 `src/api/contract.ts` 使用 `ApiSchema`、`OperationQuery`、`OperationJsonBody` 和 `OperationData`，只保留请求函数与必要的语义窄类型。

## 基础约定

开发环境中前端通过 `VITE_APP_BASE_API` 配置后端接口前缀：

```env
VITE_APP_BASE_API=/api/v1
```

本地后端默认运行在 `http://localhost:8080`。Vite 开发代理建议把 `/api` 转发到后端服务，前端业务代码只关心相对路径，例如 `/auth/login`、`/system/users`。

所有需要登录的接口都使用 Bearer Token：

```http
Authorization: Bearer <access_token>
```

## 统一响应

前端纯 HTTP 客户端位于 `ryframe-vue3/src/shared/http/client.ts`，会话副作用由 `src/app/session/sessionCoordinator.ts` 统一协调。JSON 接口默认返回：

```ts
export interface ApiResponse<T = unknown> {
  code: number
  msg: string
  data?: T
  rows?: T[]
  total?: number
}
```

约定：

- `code === 200` 表示成功。
- 错误提示统一读取 `msg`。
- 普通接口返回 `{ code, msg, data }`；分页接口返回 `{ code, msg, rows, total }`。

- 下载接口统一调用 `requestBlob` 并返回 `Promise<Blob>`；文本监控接口调用 `requestText`。二者不伪装成 JSON 包络。

分页类型建议：

```ts
export interface PageResponse<T> {
  rows: T[]
  total: number
}

export interface PageQuery {
  page?: number
  page_size?: number
  keyword?: string
}
```

前端和接口统一使用 `page` 和 `page_size`，不接受其他分页字段名。

## 认证接口

### 登录

```http
POST /auth/login
Content-Type: application/json
```

请求体：

```ts
export interface LoginRequest {
  username: string
  password: string
  captcha_id?: string
  captcha_code?: string
}
```

响应数据：

```ts
export interface LoginResult {
  access_token: string
  refresh_token?: string
  token_type?: 'Bearer'
  expires_in?: number
  user_info?: UserInfo
}
```

前端登录成功后保存 `access_token`，如果后端返回 `refresh_token` 也一并保存。后续请求自动携带 `Authorization`。

登录只校验现有账号凭据。个人修改密码、密码重置完成和租户管理员初始密码必须使用 OpenAPI `x-ryframe-password-policy` 生成的前端验证器：8-72 位可见 ASCII 字符，且至少包含大小写字母、数字和特殊字符。密码更新成功后旧会话会因 `auth_version` 变化而失效，前端应清理本地状态并重新登录。

### 刷新 Token

```http
POST /auth/refresh
Content-Type: application/json
```

请求体：

```ts
export interface RefreshTokenRequest {
  refresh_token: string
}
```

响应数据建议与登录接口保持一致，至少包含新的 `access_token`。当前前端封装会在收到 `401` 后自动调用该接口，并把并发失败请求排队等待刷新结果。

### 当前用户

```http
GET /auth/me
```

响应数据：

```ts
export interface UserInfo {
  id: string
  username: string
  nickname?: string
  email?: string
  phone?: string
  avatar?: string
  roles?: string[]
  perms?: string[]
}
```

`/auth/login` 和 `/auth/me` 只返回 `perms` 作为当前用户权限码列表。

## 动态菜单与权限

前端通过当前用户菜单树生成动态路由：

```http
GET /system/menus/current
```

菜单节点类型建议：

```ts
export interface MenuTreeNode {
  id: string
  parent_id?: string | null
  name: string
  menu_type?: 'M' | 'C' | 'F'
  icon?: string
  visible?: boolean
  status?: string
  sort?: number
  route_key?: string | null
  perm_id?: string | null
  children?: MenuTreeNode[]
}
```

字段约定：

| 字段 | 用途 |
| --- | --- |
| `name` | 菜单显示名。 |
| `menu_type` | `M` 目录、`C` 菜单、`F` 按钮。按钮不生成路由。 |
| `icon` | 菜单图标名，由前端图标组件解析。 |
| `visible` | `false`、`0`、`"0"` 表示隐藏菜单；缺省表示显示。 |
| `status` | `"1"` 表示启用，其他值前端默认不生成路由。 |
| `sort` | 菜单排序字段。 |
| `route_key` | 页面稳定标识，由前端页面注册表解析，不能使用数据库 ID。 |
| `perm_id` | 关联的 `sys_permission.id`。 |
| `children` | 子菜单树。 |

后端返回 `route_key`，前端在 `ryframe-vue3/src/router/pageRegistry.ts` 中按该稳定标识维护页面路径和组件映射。租户管理是明确的例外：路由和侧边栏入口在前端写死，并由 `tenant:list` 及对应操作权限控制。

```txt
home               -> /index
system.user        -> /system/user
system.role        -> /system/role
system.menu        -> /system/menu
system.dept        -> /system/dept
system.post        -> /system/post
system.dict        -> /system/dict
system.config      -> /system/config
system.notice      -> /system/notice
system.operlog     -> /system/operlog
system.logininfor  -> /system/logininfor
system.perm        -> /system/permission
monitor.runtime    -> /monitor/runtime
monitor.online     -> /monitor/online
monitor.server     -> /monitor/server
monitor.cache      -> /monitor/cache
monitor.db-pool    -> /monitor/db-pool
tools.gen          -> /tools/gen
```

新增页面菜单时，需要先在前端 page registry 注册 `route_key`；后端 `sys_menu` 维护菜单结构、`route_key` 和 `perm_id`。默认菜单 route-key 集合通过 OpenAPI 的 `x-ryframe-menu-routes` 扩展发布，前后端 CI 会拒绝缺失、额外或菜单类型不一致的注册项。

## 常用模块路径

前端 API 模块和后端路径建议保持以下对应关系：

| 前端模块 | 后端路径前缀 | 说明 |
| --- | --- | --- |
| `auth.ts` | `/auth` | 登录、刷新 token、当前用户和验证码。 |
| `user.ts` | `/system/users` | 用户管理、角色分配、状态和密码重置。 |
| `role.ts` | `/system/roles` | 角色管理、权限和数据范围分配。 |
| `menu.ts` | `/system/menus` | 菜单管理和当前用户菜单树。 |
| `dept.ts` | `/system/depts` | 部门管理。 |
| `post.ts` | `/system/posts` | 岗位管理。 |
| `config.ts` | `/system/configs` | 参数配置。 |
| `dict.ts` | `/system/dict` | 字典类型和字典数据。 |
| `notice.ts` | `/system/notices` | 通知公告。 |
| `permission.ts` | `/system/perms` | 权限管理。 |
| `monitor.ts` | `/monitor` | 服务、缓存、数据库连接池和指标。 |
| `generator.ts` | `/tools/gen` | 代码生成接口。 |
| `common.ts` | `/common` | 上传、下载、通用枚举等接口。 |

列表接口统一使用：

```http
GET /<module>?page=1&page_size=10&keyword=xxx
```

新增、编辑、删除建议使用：

```http
POST /<module>
PUT /<module>/{id}
DELETE /<module>/{id}
```

用户和角色当前使用逗号分隔的批量删除路径：

```http
DELETE /system/users/batch/1,2
DELETE /system/roles/batch/1,2
```

新增模块应优先使用 JSON 请求体表达批量命令；是否采用路径参数必须以 OpenAPI 为准，不自行猜测。

后端 64 位 ID 在 JSON 契约中统一使用 `string`，前端展示、路由参数和提交数据不得转为 JavaScript `number`。

## 上传、下载与导出

上传文件使用 `multipart/form-data`，不要手动设置 JSON `Content-Type`。上传接口均需登录，并会执行大小、扩展名、魔数、MD5 去重、对象存储写入和操作日志记录：

```http
POST /common/upload
```

下载和导出接口使用 Blob：

```ts
request.get('/system/users/export', {
  params,
  responseType: 'blob',
})
```

后端建议返回：

```http
Content-Type: application/vnd.openxmlformats-officedocument.spreadsheetml.sheet
Content-Disposition: attachment; filename="users.xlsx"
```

前端从 `Content-Disposition` 解析文件名；如果没有该响应头，则使用模块默认文件名。

## 监控接口

监控接口路径前缀为 `/monitor`：

| 接口 | 响应类型 | 说明 |
| --- | --- | --- |
| `GET /monitor/health` | JSON | 健康检查，可用于无需登录的探活。 |
| `GET /monitor/metrics` | `text/plain` | Prometheus 指标文本，不是统一 JSON。 |
| `GET /monitor/server` | JSON | 服务器 CPU、内存、磁盘等信息。 |
| `GET /monitor/cache` | JSON | Redis 或缓存概览。 |
| `GET /monitor/cache/commands` | JSON | 缓存命令统计。 |
| `GET /monitor/db-pool` | JSON | 主数据库连接池状态。 |
| `GET /monitor/runtime` | JSON | 主库、命名只读副本、命名业务数据源、读取策略、Redis、RustFS/对象存储和上传熔断器动态状态。 |

除健康检查和指标采集外，管理端页面应按后端权限要求携带 token 并校验 `perms`。

## 前端开发检查清单

- 新增菜单页面时，同步维护 `ryframe-vue3/src/router/pageRegistry.ts`。
- 页面按钮权限统一使用后端返回的 `perms`，不要在页面硬编码角色名。
- 表格接口统一读取 `rows` 和 `total`，新增模块不要自定义分页字段。
- 错误提示统一读取 `msg`。
- 下载接口设置 `responseType: 'blob'`，不要按统一 JSON 解析。
- 后端 64 位 ID 在前端按字符串处理。
- 菜单 `status` 只有启用值 `"1"` 才生成路由。
- 新密码表单只使用生成的 `passwordPolicy`，不要在页面复制长度、字符类别或正则。
- 提交前在 `ryframe-vue3` 目录运行 `pnpm check`，确保源码、架构、契约、Lint、类型、覆盖率和生产构建全部通过；禁止从后端根目录执行 `pnpm`。
