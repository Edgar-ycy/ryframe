# 前端集成指南

本文档面向 `ryframe-vue3` 前端开发，约定后端接口响应、认证、动态菜单、分页、上传下载和监控接口的使用方式。后续前端补模块、补类型或调整路由时，以本文档作为优先参考。

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

前端请求封装位于 `ryframe-vue3/src/api/request.ts`，默认期望后端返回统一 JSON：

```ts
export interface ApiResponse<T = any> {
  code: number
  data: T
  msg: string
  rows?: any[]
  total?: number
}
```

约定：

- `code === 200` 表示成功。
- 错误提示统一读取 `msg`。
- 普通接口返回 `{ code, msg, data }`；分页接口返回 `{ code, msg, rows, total }`。

- 下载接口使用 `responseType: 'blob'` 时直接返回原始 `AxiosResponse`，不走 JSON 业务码判断。

分页类型建议：

```ts
export interface PageResponse<T> {
  rows: T[]
  total: number
}

export interface PageQuery {
  page?: number
  pageSize?: number
  keyword?: string
}
```

前端统一使用 `page` 和 `pageSize`。如果后端内部支持 `page_size`，也应该在接口层兼容 `pageSize`，避免前端模块各自转换。

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
  id: string | number
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

前端通过用户菜单树生成动态路由：

```http
GET /system/menus/user-tree
```

菜单节点类型建议：

```ts
export interface MenuTreeNode {
  id: string | number
  parent_id?: string | number | null
  name?: string
  menu_name?: string
  menu_type?: 'M' | 'C' | 'F'
  icon?: string
  visible?: boolean | string | number
  status?: string | number
  sort?: number
  order_num?: number
  children?: MenuTreeNode[]
}
```

字段约定：

| 字段 | 用途 |
| --- | --- |
| `name` / `menu_name` | 菜单显示名，前端优先读取 `name`，兼容 `menu_name`。 |
| `menu_type` | `M` 目录、`C` 菜单、`F` 按钮。按钮不生成路由。 |
| `icon` | 菜单图标名，由前端图标组件解析。 |
| `visible` | `false`、`0`、`"0"` 表示隐藏菜单；缺省表示显示。 |
| `status` | `"1"` 表示启用，其他值前端默认不生成路由。 |
| `sort` / `order_num` | 菜单排序字段，优先使用 `sort`，兼容 `order_num`。 |
| `children` | 子菜单树。 |

后端不再返回菜单路由字段。前端在 `ryframe-vue3/src/router/pageRegistry.ts` 中按菜单 ID 维护页面路径和组件映射，当前已注册页面包括：

```txt
0  -> /index
1  -> /system
2  -> /monitor
3  -> /tools
4  -> /system/user
5  -> /system/role
6  -> /system/menu
7  -> /system/dept
8  -> /system/post
9  -> /system/dict
10 -> /system/config
11 -> /system/notice
12 -> /system/operlog
13 -> /system/logininfor
14 -> /monitor/runtime
15 -> /monitor/online
16 -> /monitor/server
17 -> /tools/gen
23 -> /monitor/cache
24 -> /monitor/db-pool
25 -> /system/permission
```

新增页面菜单时，需要先在前端 page registry 注册菜单 ID；后端 `sys_menu` 只维护结构字段。

## 常用模块路径

前端 API 模块和后端路径建议保持以下对应关系：

| 前端模块 | 后端路径前缀 | 说明 |
| --- | --- | --- |
| `auth.ts` | `/auth` | 登录、刷新 token、当前用户、验证码。 |
| `user.ts` | `/system/users` | 用户管理。 |
| `role.ts` | `/system/roles` | 角色管理、角色授权。 |
| `menu.ts` | `/system/menus` | 菜单管理和用户菜单树。 |
| `dept.ts` | `/system/depts` | 部门管理。 |
| `post.ts` | `/system/posts` | 岗位管理。 |
| `config.ts` | `/system/configs` | 参数配置。 |
| `dict.ts` | `/system/dicts` | 字典类型和字典数据。 |
| `notice.ts` | `/system/notices` | 通知公告。 |
| `permission.ts` | `/system/permissions` | 权限管理。 |
| `monitor.ts` | `/monitor` | 服务、缓存、数据库连接池和指标。 |
| `tools.ts` | `/tools` | 代码生成等工具接口。 |
| `common.ts` | `/common` | 上传、下载、通用枚举等接口。 |

列表接口统一使用：

```http
GET /<module>?page=1&pageSize=10&keyword=xxx
```

新增、编辑、删除建议使用：

```http
POST /<module>
PUT /<module>/{id}
DELETE /<module>/{id}
```

批量删除建议使用：

```http
DELETE /<module>/batch
Content-Type: application/json

{ "ids": ["1", "2"] }
```

如果后端 ID 是 64 位整数，前端类型请使用 `string | number`，展示和提交时优先按字符串处理，避免 JavaScript 精度问题。

## 上传、下载与导出

上传文件使用 `multipart/form-data`，不要手动设置 JSON `Content-Type`。上传接口均需登录，并会触发魔数校验、MD5 去重、审计事件和后台任务：

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
| `GET /monitor/db-pool` | JSON | 数据库连接池状态。 |
| `GET /monitor/runtime` | JSON | 功能开关、消息队列、任务队列和上传熔断器状态。 |

除健康检查和指标采集外，管理端页面应按后端权限要求携带 token 并校验 `perms`。

## 前端开发检查清单

- 新增菜单页面时，同步维护 `ryframe-vue3/src/router/pageRegistry.ts`。
- 页面按钮权限统一使用后端返回的 `perms`，不要在页面硬编码角色名。
- 表格接口统一读取 `rows` 和 `total`，新增模块不要自定义分页字段。
- 错误提示统一读取 `msg`。
- 下载接口设置 `responseType: 'blob'`，不要按统一 JSON 解析。
- 后端 64 位 ID 在前端按字符串处理。
- 菜单 `status` 只有启用值 `"1"` 才生成路由。
