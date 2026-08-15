# RyFrame API 使用指南

> 最后核对：2026-08-13
> API 版本：`v1`

本文档说明稳定约定和常见流程。所有路径、请求字段和响应 Schema 的唯一事实来源是 OpenAPI；运行时文档与仓库中的 `openapi/openapi.json` 必须精确一致：

```text
GET /api/v1/api-docs/openapi.json
GET /api/v1/swagger-ui
```

Swagger UI 的 HTML、CSS、JavaScript、字体和图标均由 Rust crate 在编译期内嵌，浏览器不会从 CDN 或其他外部站点加载资源。根包的默认 `runtime-swagger-ui` feature 只为开发和受控测试启用该资源；`/api/v1/swagger-ui` 是唯一页面入口，不提供尾斜杠或 `index.html` 兼容路由；静态资源位于 `/api/v1/swagger-ui/{asset}`。文档页面的 CSP 保持 `script-src 'self'`，仅因 Swagger UI 运行时组件使用内联样式而对该 HTML 响应设置 `style-src 'self' 'unsafe-inline'`。

以上运行时文档只用于开发和受控测试环境。生产配置强制
`APP_API_DOCS_ENABLED=false`，生产构建使用 `cargo build --release --no-default-features`
（或等价的镜像构建命令）排除整个 Swagger UI 资源；公网 Nginx 也应返回 `404`。未编译
`runtime-swagger-ui` 时若设置 `APP_API_DOCS_ENABLED=true`，API 会在连接 MySQL、Redis 或
对象存储之前明确启动失败。生产排查使用仓库中由 CI 重新生成并精确比对的 `openapi/openapi.json`。

## 1. 基础约定

默认前缀：

```text
/api/v1
```

JSON 接口使用 `Content-Type: application/json`。受保护接口携带：

```http
Authorization: Bearer <access_token>
```

登录前需要选择租户时携带：

```http
X-Tenant-Id: <tenant_id>
```

认证成功后，租户身份以签名 Token 中的值为准，请求头不能覆盖它。

## 2. 响应模型

### 普通响应

```json
{
  "code": 200,
  "message": "操作成功",
  "data": {},
  "request_id": "0198f7e8-0000-7000-8000-000000000001",
  "error_key": null,
  "details": null
}
```

`code` 与 HTTP 状态码一致；`request_id` 与 `X-Request-Id` 响应头一致，且为 UUID v7。无数据的成功响应返回 `data: null`。

### 分页响应

```json
{
  "code": 200,
  "message": "查询成功",
  "data": {
    "items": [],
    "page": 1,
    "page_size": 20,
    "total": 0,
    "total_pages": 0,
    "max_page_size": 100
  },
  "request_id": "0198f7e8-0000-7000-8000-000000000001",
  "error_key": null,
  "details": null
}
```

统一分页参数：

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `page` | integer | 页码，从 1 开始 |
| `page_size` | integer | 每页数量，受服务端上限约束 |

### 标识符（ID）

所有 HTTP ID 都是十进制字符串：

```json
{ "id": "1958123456789012345" }
```

前端不得将 ID 转成 JavaScript `number`，否则 Snowflake 64 位整数可能丢失精度。

### 错误

错误仍使用统一 JSON 包装，HTTP 状态与错误类型一致：

| HTTP 状态 | 含义 |
| ---: | --- |
| `400` | JSON、查询参数或业务校验失败 |
| `401` | Token 缺失、失效、被撤销或会话版本过期 |
| `403` | CSRF 校验失败、权限不足、租户不可用或数据范围不允许 |
| `404` | 资源不存在 |
| `409` | 幂等冲突、refresh 并发冲突或资源状态冲突 |
| `413` | 上传或请求体超过服务端限制 |
| `429` | 限流；响应携带 `Retry-After` |
| `501` | 功能开关未启用（`error_key=feature_disabled`），例如服务账号相关管理接口 |
| `503` | Redis、数据库或必要对象存储暂不可用 |
| `500` | 未预期的服务端错误 |

客户端应优先按稳定的 `error_key` 映射本地化文案，未命中时回退到服务端 `message`，同时保留 HTTP 状态和 `request_id` 用于排障。`details` 仅包含可安全公开的结构化参数。

### 重试、幂等和代理边界

认证后的 `/system`、`/platform` 写请求可以携带 `Idempotency-Key`。存储键只隔离租户、用户和客户端提供的原始键；请求指纹完整绑定 HTTP 方法、真实规范化路径、排序后的查询参数以及 body SHA-256。同一主体复用同一个键且指纹一致时，处理中请求返回 `409` 与 `Retry-After`，完成结果保留 300 秒并可回放；方法、路径、查询值或正文任一不同都会返回 `409`，仅查询参数顺序不同仍视为同一请求。超过 1 MiB 的成功响应不会被缓存，后续重复请求返回不可回放冲突，但首次成功响应保持不变。认证、上传下载、生成器、监控和流式响应不参与幂等缓存。

API 不定义 `X-Nonce` / `X-Timestamp` 通用防重放协议，客户端不得依赖或发送这两个头。未签名且由客户端自行生成的 nonce 与时间戳不能证明请求来源，也没有绑定主体、方法、目标路径和 body；把它们当作安全校验会产生错误的保护预期。当前浏览器边界由 HTTPS、Bearer 身份与授权、签名 CSRF challenge、refresh family 原子轮换以及上述幂等绑定共同承担。未来若为外部机器客户端增加应用层持有者证明，必须设计独立的密钥注册与轮换流程，并采用 [RFC 9421 HTTP Message Signatures](https://www.rfc-editor.org/rfc/rfc9421) 一类可验证签名，明确覆盖方法、目标 URI、内容摘要、创建时间与 nonce；不得恢复裸双头方案。

应用只在 socket 对等端属于 `[proxy].trusted_cidrs` 时解析转发头；直连客户端发送的 XFF 不会影响审计、限流或登录保护。生产 CORS 必须显式列出管理端完整 Origin，空列表表示拒绝跨域，不表示允许任意来源。

## 3. 认证流程

### 验证码

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/v1/auth/captcha/config` | 查询验证码是否启用 |
| `GET` | `/api/v1/auth/captcha/generate` | 生成验证码数据 |
| `GET` | `/api/v1/auth/captcha/image` | 获取验证码图片 |
| `POST` | `/api/v1/auth/captcha/verify` | 独立校验验证码 |

验证码使用 Redis 时，写入或一次性校验失败会直接返回 `503`，不会伪装成验证码错误；开发环境显式使用内存模式时不依赖 Redis。

### 登录和令牌

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/v1/auth/csrf` | 签发 5 分钟 CSRF challenge，并设置 challenge Cookie |
| `POST` | `/api/v1/auth/login` | 登录；JSON 只返回 access token，refresh token 只写入 Cookie |
| `POST` | `/api/v1/auth/refresh` | 空请求体，通过 refresh Cookie 与 CSRF challenge 轮换会话 |
| `POST` | `/api/v1/auth/logout` | 通过 Cookie 与 CSRF 撤销整个 refresh family；Bearer 可选 |
| `GET` | `/api/v1/auth/me` | 获取当前主体 |
| `GET` | `/api/v1/auth/sessions` | 查询当前租户、当前用户的有效登录设备 |
| `DELETE` | `/api/v1/auth/sessions/{sid}` | 撤销当前用户的指定设备会话 |
| `POST` | `/api/v1/auth/sessions/revoke-others` | 撤销当前用户除本设备外的全部设备会话 |
| `POST` | `/api/v1/auth/password-reset/complete` | 使用一次性 Token 完成密码重置 |

登录前先获取 challenge。响应带有 `Cache-Control: no-store`，JSON 中的 `csrf_token` 只保存在页面内存：

```http
GET /api/v1/auth/csrf
X-Tenant-Id: system
```

登录示例：

```http
POST /api/v1/auth/login
X-Tenant-Id: system
X-CSRF-Token: <csrf_token>
Content-Type: application/json
Cookie: <csrf_challenge_cookie>

{
  "username": "admin",
  "password": "********",
  "captcha_id": "...",
  "captcha_code": "..."
}
```

成功响应的业务数据只有 `access_token`、`expires_in` 和 `user_info`。refresh token 永远不出现在 JSON、日志或 OpenAPI Schema 中，只通过名为 `ryframe_refresh_token` 的 host-only Cookie 下发；Cookie 属性固定为 `HttpOnly`、`SameSite=Lax`、`Path=/api/v1/auth`，生产环境强制 `Secure`。会话从登录起最多存活 7 天，刷新不会延长这个绝对期限。

刷新接口没有请求体，也不接收 `X-Tenant-Id`：

```http
POST /api/v1/auth/refresh
X-CSRF-Token: <csrf_token>
Cookie: ryframe_refresh_token=<opaque_token>; <csrf_challenge_cookie>
```

refresh 成功会原子轮换 Cookie 和 `jti`，稳定设备会话标识为 `sid`。同一枚旧 token 在 5 秒并发窗口内返回 `409` 和 `Retry-After`；窗口外再次使用会被判定为重放，整个 refresh family 被撤销并返回 `401`。Redis 不可用时返回 `503`，服务端不会清除仍可能有效的 Cookie。

access token 只用于业务请求并由页面内存持有。客户端遇到业务 `401` 时最多执行一次 single-flight 刷新并重放原请求；`503` 表示服务暂不可用，不能被当作匿名状态。登出即使 access token 已过期也可撤销 refresh family，重复调用保持成功。

### 登录设备

三个登录设备接口都要求有效的 Bearer access token。`GET /api/v1/auth/sessions` 不要求
CSRF，只返回当前租户、当前用户且仍由 Refresh Family 判定为有效的设备，并把当前 `sid`
标记为 `current=true`。响应包含浏览器、操作系统、IP、登录位置、登录时间、最近活动时间和
绝对过期时间；在线用户 metadata 仅用于展示，缺少 metadata 的 Refresh Family 不会被伪造为
设备记录。

`DELETE /api/v1/auth/sessions/{sid}` 与
`POST /api/v1/auth/sessions/revoke-others` 是 Cookie 认证边界内的写操作，除 Bearer token 外还
必须携带由 `/api/v1/auth/csrf` 签发、并与当前 `sid` 绑定的 challenge Cookie 和
`X-CSRF-Token`。撤销当前 `sid` 会同时清除本浏览器的认证 Cookie；撤销其他设备不会退出
当前设备。批量接口始终排除当前 `sid`，响应中的 `revoked_count` 是本次实际从活跃变为撤销的
会话数量。

会话接口的状态语义如下：

- Bearer token 缺失、无效或对应 Refresh Family 已撤销时返回 `401`。
- CSRF challenge 缺失、无效或与当前 `sid` 不匹配时返回 `403`。
- 单设备 `sid` 不存在、已过期、跨租户或属于其他用户时统一返回 `404`；已完成的同身份撤销
  可以幂等返回成功。
- Redis 会话或索引不可用、响应损坏时返回 `503`，不得把它解释为设备不存在或撤销成功。
- 每个租户用户最多保有 256 个活跃设备会话；达到上限后新登录返回 `409`。批量撤销也只接受
  最多 256 个候选。升级遗留或异常索引超过该边界时，批量接口返回 `400`，客户端或运维人员
  应根据会话列表逐一调用 `DELETE /api/v1/auth/sessions/{sid}`，不能直接删除 Redis 键。

Refresh Family 是会话是否有效的唯一事实来源。租户索引、租户用户索引和在线用户 metadata
只负责发现候选与展示；服务端在返回列表或执行撤销前都会重新校验 Family 的租户、用户、撤销
状态和绝对过期时间。升级兼容期内会同时读取新索引和旧在线用户 metadata，并逐项回查
Refresh Family；该双读至少保留一个最大 Refresh TTL，即七天。

### 新密码策略

个人修改密码、密码重置完成和租户管理员初始密码使用同一策略：

- 长度为 8-72 个字符。
- 仅允许可见 ASCII 字符，不允许空格。
- 至少包含一个大写字母、一个小写字母、一个数字和一个特殊字符。

策略由 OpenAPI 顶层扩展 `x-ryframe-password-policy` 发布，各密码字段同时声明等价的 `minLength`、`maxLength` 和 `pattern`。前端必须从该扩展生成验证配置，不维护第二份正则。个人修改密码和重置密码成功后，服务端会递增用户 `authorization_version`，此前签发的 access/refresh token 会失效。

管理员不能直接设置用户新密码。标准流程是：

1. `POST /api/v1/system/users/{id}/password-reset-requests` 发起重置。
2. 将返回的一次性链接交给目标用户。
3. 用户调用 `/api/v1/auth/password-reset/complete` 设置新密码。
4. 服务端更新会话版本，使旧 access/refresh token 失效。

## 4. Canonical 路径

资源接口遵守统一形式：

| 操作 | 形式 |
| --- | --- |
| 分页列表 | `GET /resources` |
| 详情 | `GET /resources/{id}` |
| 创建 | `POST /resources` |
| 更新 | `PUT /resources/{id}` |
| 删除 | `DELETE /resources/{id}` |
| 领域动作 | `/resources/{id}/action` 或资源级动作路径 |

项目不提供无上限列表；表格查询必须分页，选择器必须使用有限候选接口。项目不保留旧接口别名，状态修改和缓存刷新等动作只使用当前 OpenAPI 声明的资源路径。

角色和用户候选项分别使用 `GET /api/v1/system/roles/options` 与 `GET /api/v1/system/users/options`。两者只接受可选的 `q` 和 `limit`：`q` 会去除首尾空白并执行当前租户内的前缀搜索，最长 64 个字符；`limit` 默认使用分页配置的默认页大小，必须位于 `1..=max_page_size`，非法值返回 `400`。响应固定为：

```json
{
  "items": [
    {
      "value": "角色或用户 ID",
      "label": "显示名称",
      "description": "可选说明",
      "disabled": false
    }
  ],
  "has_more": false
}
```

候选接口采用 `limit + 1` 判断 `has_more`，不执行总数统计，也不返回 `page`、`page_size` 或 `total`。

## 5. 模块目录

下表用于快速定位，具体字段和权限码查看 Swagger UI。

| 前缀 | 模块 | 额外动作 |
| --- | --- | --- |
| `/api/v1/system/users` | 用户 | `/options`、`PUT /{id}/roles`、`PUT /{id}/status`、`/batch/{ids}`、导入模板、导出和重置请求 |
| `/api/v1/system/user-imports` | 异步用户导入 | 幂等创建、列表、详情、取消、异常行和错误报告 |
| `/api/v1/system/authorization-diagnostics` | 权限诊断 | `GET /users/{id}` 从主库重算用户最终授权 |
| `/api/v1/system/config-packages` | 租户配置包 | 幂等生成、列表、详情和下载 |
| `/api/v1/system/config-transfers` | 租户配置迁移 | 上传、从已有包创建、预览、应用、回滚和明细 |
| `/api/v1/system/roles` | 角色 | `/options`、`GET/PUT /{id}/permissions`、`PUT /{id}/data-scope` |
| `/api/v1/system/perms` | 权限 | `/tree`、`/sync` |
| `/api/v1/system/menus` | 菜单 | `/tree`、`/current` |
| `/api/v1/system/depts` | 部门 | `/tree` |
| `/api/v1/system/posts` | 岗位 | `/exports` |
| `/api/v1/system/configs` | 参数配置 | `/key/{key}`、`DELETE /cache`、`/exports` |
| `/api/v1/system/dict` | 字典 | `/types`、`/data`、`/data/type/{dict_type}`、`/types/exports` |
| `/api/v1/system/notices` | 通知公告 | `POST /{id}/publish-message` |
| `/api/v1/system/messages` | 消息中心 | `/unread-count`、`POST /ack`、`POST /delete`、`PUT /{id}/read`、`PUT /read-all` |
| `/api/v1/system/operlogs` | 操作日志 | `/exports` |
| `/api/v1/system/loginlogs` | 登录日志 | `/exports` |
| `/api/v1/system/online` | 在线用户 | `DELETE /{sid}`；`sid` 精确表示一个设备会话 |
| `/api/v1/system/service-accounts` | 服务账号 | 账号 CRUD、角色整体替换、API Key 列表/创建/撤销 |
| `/api/v1/system/service-delegations` | 服务委托管理 | 当前租户委托分页和管理员撤销 |
| `/api/v1/system/service-access-audits` | 服务访问审计 | Agent 成功、拒绝和错误记录分页 |
| `/api/v1/profile/service-delegations` | 个人服务委托 | 本人委托、可委托能力、创建和撤销 |
| `/api/v1/agent/v1` | Agent API | 固定能力发现、用户/部门/岗位目录和字典只读查询 |
| `/api/v1/platform/tenants` | 租户 | 兼容列表、`/page`、`GET /{tenant_id}`、`/{tenant_id}/usage`、`PUT /{tenant_id}/status` |
| `/api/v1/auth/profile` | 个人中心 | `/password`、`/avatar` |
| `/api/v1/tools/gen` | 代码生成 | `/tables`、`/preview`、`/generate`、`/download` |
| `/api/v1/common/upload` | 文件上传 | `/image`、`/avatar` |
| `/api/v1/common/file` | 文件 | `/download` |
| `/api/v1/common/jobs` | 我的导出任务 | 最近任务、未读完成提醒、取消和下载 |
| `/api/v1/monitor` | 监控 | `/overview`、`/overview/trends`、`/retention`、`/jobs`、`/schedules`、`/metrics`、`/server`、`/cache`、`/db-pool`、`/runtime`；探针位于根路径 `/livez`、`/readyz` |

公告创建、更新和响应只使用 `content_markdown`，不接受旧 `content` 字段。Markdown 原文按 UTF-8 字节校验，允许 1–60,000 字节；限制由 OpenAPI 的 `x-ryframe-notice-policy` 发布，前端不得复制常量。

消息中心的 `acked_at` 代表客户端已实际收到消息，客户端通过 WebSocket 或收件箱补拉后自动
调用 `POST /api/v1/system/messages/ack`；它不是人工操作。用户打开详情时才调用
`PUT /api/v1/system/messages/{id}/read`，服务端同时保证该记录已送达。用户可以用
`POST /api/v1/system/messages/delete` 提交 1–100 个字符串 ID 删除自己的收件箱记录；重复
删除幂等，跨租户、跨用户和不存在的 ID 不会影响其他数据。公告和通知正文均为 Markdown
原文，客户端必须禁用原始 HTML 并在渲染前净化输出。

异步导出创建成功只表示任务已进入队列，不表示文件已经生成。任务进入 `succeeded` 或 `failed`
后会产生当前申请人的持久未读提醒；客户端通过
`GET /api/v1/common/jobs/notifications/unread-count` 展示“我的导出任务”徽标，用户打开任务中心后
调用 `POST /api/v1/common/jobs/notifications/read` 提交本次实际看到的 1–100 个任务字符串 ID，幂等
确认对应提醒。已读状态保存在服务端，
不会因刷新页面、重新登录或切换浏览器标签页而恢复为未读；取消任务不产生完成提醒。
升级迁移会把部署前已经成功或失败的历史导出初始化为已查看，避免版本上线时集中产生旧提醒。
未读统计与任务中心保持同一个“当前租户、当前账号最近 100 条”可见范围，不会为任务中心已经无法
展示的更早历史记录保留无法清除的徽标。

### 当前用户菜单

登录后使用：

```text
GET /api/v1/system/menus/current
```

后端只返回稳定 `route_key`、菜单元数据和权限。前端必须通过本地页面注册表解析 `route_key`，不得执行服务端下发的任意组件路径。

### 异步用户导入

旧同步路径 `POST /api/v1/system/users/import` 已删除。创建导入改为：

```text
POST /api/v1/system/user-imports
```

请求必须携带 `Idempotency-Key`，并使用 `multipart/form-data` 上传唯一的 `file` 字段。只接受真实 `.xlsx` 文件；默认上限为 10 MiB、20,000 行，同租户默认只允许一个等待或运行中的任务。创建成功返回 `202`。客户端通过列表、详情和异常行接口查询进度，使用 `POST /{id}/cancel` 申请在批次边界取消，并在报告就绪后通过 `GET /{id}/report` 下载私有 Excel 报告。

重复用户名固定跳过且不更新已有资料。新用户处于待激活状态且不自动分配角色。导入是部分成功操作，取消、申请人停用或撤权不会回滚已经提交的批次。源文件、任务载荷、错误报告和行数据不得写入客户端日志或操作审计参数。

上传前必须通过 `GET /api/v1/system/users/import-template` 下载当前用户可用的最新模板。第一工作表只接受按顺序排列的 `用户名`、`昵称`、`邮箱`、`手机号`、`部门完整路径` 五列；未知列、重复列、缺失列、顺序变化以及旧版 `部门ID` 列都会被拒绝。部门路径从模板的“可用部门”工作表复制，不填写任何数据库 ID；系统在每个处理批次重新校验路径唯一性、部门状态和申请人的当前数据范围。

### 数据保留、权限诊断和运维总览

数据保留接口只允许 `system` 租户访问。`POST /api/v1/monitor/retention/preview` 只统计候选记录；`POST /api/v1/monitor/retention/run` 是永久硬删除后台任务的幂等入队入口，必须先由管理员核对预览结果。普通租户不能通过总览或保留接口读取其他租户明细。

权限诊断使用 `GET /api/v1/system/authorization-diagnostics/users/{id}`，只读展示调用者当前数据范围内用户的角色、权限来源、菜单、最终数据范围和版本同步状态。Redis 不可用时主库诊断仍可返回，不提供提权、修改纪元或清缓存入口。

运维总览使用 `GET /api/v1/monitor/overview` 和 `GET /api/v1/monitor/overview/trends?range=6h|24h|7d`。趋势严格按当前租户聚合；系统租户只额外包含无租户的平台后台任务，不会统计其他普通租户。详细运行边界见[数据生命周期、异步导入与运维诊断](data-lifecycle.md)。

### 平台租户容量与配额

平台租户接口只允许 `system` 租户访问。原有 `GET /api/v1/platform/tenants` 继续返回兼容的基础列表，
不增加用量字段；新接口为：

```text
GET /api/v1/platform/tenants/page
GET /api/v1/platform/tenants/{tenant_id}
GET /api/v1/platform/tenants/{tenant_id}/usage
```

分页接口要求 `tenant:list`，默认每页 20 条，最大 100 条，支持按租户标识、租户名称、启停状态、
到期状态和容量状态筛选。启停状态为 `enabled`、`disabled`；到期状态为 `active`、`expiring`、
`expired`、`never`；容量状态为 `normal`、`warning`、`critical`、`exceeded`、`unlimited`、
`unknown`。只有同时具有 `tenant:usage:list` 的调用者才会在分页和详情响应中得到
`capacity_status` 与 `usage`；缺少该权限时二者为 `null`，并且不能使用容量状态筛选。独立用量接口
固定要求 `tenant:usage:list`。两个权限都只属于系统租户，迁移不会自动把用量权限授予任何普通角色；
系统租户管理员可按职责显式授权平台管理角色，普通租户访问上述接口返回 `403`。

用户、角色和存储用量均从主库强一致读取。用户统计所有未软删除、仍占用席位的用户，角色统计所有
未软删除角色，存储按未软删除文件记录的 `file_size` 汇总，不扫描对象存储目录。三类资源的配额值
为 `0` 时表示无限制，不表示禁止创建或上传。单项与整体状态按实际使用率计算：低于 80% 为
`normal`，80% 至不足 90% 为 `warning`，90% 至不足 100% 为 `critical`，达到或超过 100% 为
`exceeded`；三项全部无限制时整体为 `unlimited`。整体容量状态只由这三类主库资源决定，不受 Redis
请求窗口影响。

`usage.request_window` 只表示当前一分钟限流窗口，包含当前计数、上限和剩余秒数，不是历史请求量。
租户请求上限为 `0` 或全局租户限流关闭时，该项为 `unlimited`；Redis 读取失败时，该项单独变为
`unknown`，用户、角色、存储和辅助汇总仍正常返回。辅助汇总只提供 Pending、Running、Dead 后台
任务数、启用计划数、活动用户导入数以及 Cron 开关，不返回任务载荷、用户明细或错误全文。接口不
自动轮询，管理端只应在页面进入、筛选、翻页、打开详情或手动刷新时读取。

### 服务账号、个人委托和 Agent API

服务账号是租户内独立于普通用户登录的机器主体，只能调用编译期固定的 Agent 只读能力。管理接口
继续使用普通 Bearer 会话，并按以下权限分离查看、编辑、角色授权、Key 轮换、Key 撤销、委托管理
和访问审计；迁移只安装权限与 `system.service-accounts` 菜单，不自动授予普通角色。

```text
GET    /api/v1/system/service-accounts
POST   /api/v1/system/service-accounts
GET    /api/v1/system/service-accounts/{id}
PUT    /api/v1/system/service-accounts/{id}
PUT    /api/v1/system/service-accounts/{id}/status
DELETE /api/v1/system/service-accounts/{id}
GET    /api/v1/system/service-accounts/{id}/roles
PUT    /api/v1/system/service-accounts/{id}/roles
GET    /api/v1/system/service-accounts/{id}/credentials
POST   /api/v1/system/service-accounts/{id}/credentials
DELETE /api/v1/system/service-accounts/{id}/credentials/{credential_id}
GET    /api/v1/system/service-delegations
DELETE /api/v1/system/service-delegations/{id}
GET    /api/v1/system/service-access-audits
```

创建 API Key 必须携带 16–128 个可见 ASCII 字符组成的 `Idempotency-Key`。完整 Key 形如
`rfk_<key_id>.<secret>`，只在首次成功响应的 `secret` 字段显示；响应带 `Cache-Control: no-store`
和 `Pragma: no-cache`。同一主体以相同幂等键和相同请求重试只返回同一凭据元数据，`secret` 为
`null`；相同幂等键绑定不同请求时返回 `409`。列表只返回 Key ID、标签、状态、到期和撤销等元数据，
数据库也不保存明文 Secret。默认每个账号最多同时保有两把有效 Key，每把有效期不超过 90 天，
便于先创建新 Key、切换调用方，再撤销旧 Key。

个人委托允许已登录用户把自己和指定服务账号共同拥有的固定能力显式授予该账号：

```text
GET    /api/v1/profile/service-delegations
GET    /api/v1/profile/service-delegations/capabilities
POST   /api/v1/profile/service-delegations
DELETE /api/v1/profile/service-delegations/{id}
```

创建委托同样要求 `Idempotency-Key`。委托令牌形如 `rfd_<secret>`，只在首次成功响应的 `token`
字段显示，幂等重放时为 `null`，并使用相同的禁止缓存响应头。未指定到期时间时默认 24 小时，最大
不超过 30 天。用户只能撤销本人委托；具备管理权限的管理员可从租户委托列表撤销。能力候选来自
服务端固定注册表，并且必须同时存在于当前用户和服务账号的有效权限中，客户端不能提交任意路径、
operationId 或权限码来扩展能力。

Agent 请求不使用普通 JWT。直接调用只携带 API Key；代表用户调用还必须携带委托令牌：

```text
Authorization: RyFrameApiKey rfk_<key_id>.<secret>
X-RyFrame-Delegation: rfd_<secret>     # 仅委托模式
```

当前注册的只读入口只有：

```text
GET /api/v1/agent/v1/capabilities
GET /api/v1/agent/v1/directory/users?page=1&page_size=20
GET /api/v1/agent/v1/directory/departments?page=1&page_size=20
GET /api/v1/agent/v1/directory/posts?page=1&page_size=20
GET /api/v1/agent/v1/reference/dictionaries/{type_code}?page=1&page_size=20
```

`capabilities` 只返回调用方在当前时刻真正可用的注册能力，本身不授予数据权限。直接模式按服务账号
的角色权限与数据范围执行；委托模式还要求服务账号权限、被代表用户权限和委托能力白名单三者同时
允许，用户与服务账号的数据范围取交集。用户和部门目录按该交集做行级过滤；岗位和字典只有双方
数据范围均为“全部”时才返回记录。分页默认 20，部署可把最大页大小设为不超过 100；不接受未注册
过滤或排序参数。所有 Agent 响应在数据库计算出的权限事实下执行，并受固定查询超时和最大响应字节
限制。

Agent 的成功、拒绝、未知路径和运行错误都进入服务访问审计。成功查询与对应审计在同一个主库事务
提交后才返回结果；失败审计无法写入时返回 `503`，不会把未审计请求伪装成成功。审计保存 request ID、
operationId、能力、访问模式、结果、原因、状态码、行数、响应字节与授权版本，不保存 API Key、
委托令牌、响应正文、原始 IP 或原始 user-agent。`401` 不区分 Key、Secret 或委托令牌的具体错误；
`403` 表示固定能力或双主体交集不允许；`413` 表示最终响应超过上限；`429` 携带 `Retry-After`；
Redis、数据库、审计或查询时限不可用时返回 `503`。

### 租户配置包迁移

配置包接口：

```text
POST /api/v1/system/config-packages
GET  /api/v1/system/config-packages
GET  /api/v1/system/config-packages/{id}
GET  /api/v1/system/config-packages/{id}/download
```

迁移接口：

```text
POST /api/v1/system/config-transfers/upload
POST /api/v1/system/config-transfers/from-package
GET  /api/v1/system/config-transfers
GET  /api/v1/system/config-transfers/{id}
GET  /api/v1/system/config-transfers/{id}/items
POST /api/v1/system/config-transfers/{id}/preview
POST /api/v1/system/config-transfers/{id}/apply
POST /api/v1/system/config-transfers/{id}/rollback
```

生成、上传、从已有包创建、预览、应用和回滚都是显式操作；所有 `POST` 必须携带
`Idempotency-Key`，首次接受返回 `202`。上传使用 `multipart/form-data`，只允许一个名为
`file` 的 `*.ryframe-config.zip` 文件。默认压缩包上限为 5 MiB、解压后上限为 20 MiB、
最多 10,000 项。服务端只接受 ZIP 根目录下的 `manifest.json` 和 `resources.json`，并对路径、
文件数、压缩比、JSON 深度、SHA-256、稳定业务键及目标端引用执行校验。

预览不会修改目标配置，而是返回基于目标主库 `configuration_version`、
`authorization_epoch` 和当前资源生成的计划；项目分类为 `create`、`update`、`unchanged`、
`conflict` 和 `blocked`。应用请求必须回传预览的 `plan_hash`、目标配置版本和目标授权纪元；
任一值过期返回 `409`，客户端必须重新预览。应用前自动生成目标快照；应用后默认有 168 小时
回滚窗口。回滚仍会复核版本、快照、后续人工修改和新增引用，不提供部分回滚。

配置包不接受或返回数据库 ID 作为资源稳定键，也不包含用户、密码、Secret、Token、文件、日志、
任务或应用配置。部门使用完整名称路径，岗位、权限和角色使用代码，目录与页面使用
`route_key`，参数只有 `portable=true` 且不命中敏感键规则时才进入配置包。完整规则见
[租户配置包迁移](tenant-config-transfer.md)。

### 角色分配

用户角色和角色权限均采用全量替换语义：

```text
PUT /api/v1/system/users/{id}/roles
PUT /api/v1/system/roles/{id}/permissions
PUT /api/v1/system/roles/{id}/data-scope
```

调用前先读取当前值，提交完整目标集合，不要只提交增量差异。创建用户时可直接提交 `role_ids`，用户和角色关联在同一数据库事务中创建；后续资料、角色和状态分别通过用户资源、`/{id}/roles` 和 `/{id}/status` 更新。数据范围请求同时提交 `data_scope` 和 `dept_ids`，两者在同一数据库事务中替换。

角色权限、数据范围、菜单和权限目录变更会提升租户授权纪元。角色成员无需重新登录：下一次受保护请求会使旧授权快照失效并从主库重建。在线管理端通过 WebSocket `authorization_changed` 控制帧立即刷新权限、菜单和动态路由；所有受保护响应同时返回 `X-Authorization-Epoch`，作为实时通知断线时的校准兜底。撤销权限后即使旧页面按钮短暂存在，后端也会立即拒绝无权请求。

### 参数配置

按 key 查询：

```text
GET /api/v1/system/configs/key/sys.account.captchaEnabled
```

清空参数缓存：

```text
DELETE /api/v1/system/configs/cache
```

### 文件上传和下载

上传使用 `multipart/form-data`，文件字段名和 bucket 约束以 OpenAPI 为准。普通文件上限 10 MiB，头像上限 5 MiB，上传超时 120 秒；服务端执行类型、大小、魔数、去重和熔断校验，并记录文件元数据。固定长度和 chunked 请求超限都返回 `413`。

对象存储熔断、传输错误、`429` 或上游 `5xx` 返回 `503`；下载仅在数据库元数据或底层对象真实不存在时返回 `404`。

上传、头像和下载都属于私有资源。下载只接受服务端允许的 bucket 和相对对象路径，必须携带有效 Bearer，不接受任意本地文件系统路径。

## 6. 权限和数据范围

Handler 通过 `#[perm("...")]` 声明权限码。超级管理员规则、角色权限和数据范围由服务端统一校验，前端权限按钮只改善体验，不是安全边界。

常见权限码形式：

```text
system:user:list
system:user:add
system:user:edit
system:user:remove
```

数据范围作用于用户、部门、公告和日志等查询。即使用户拥有接口权限，也只能读取主体数据范围允许的记录。

## 7. DTO 契约规则

- 写入 DTO 默认拒绝未知字段；拼错字段会返回 `400`，不会被静默忽略。
- 状态、长度、邮箱、手机号和密码规则由服务端校验。
- 空字符串与 `null` 含义不同，调用方应按 OpenAPI Schema 发送。
- API v1 内进行破坏性重构时不保留旧路径，前后端必须在同一变更窗口更新。
- API 模块的字段或路径变更必须更新 OpenAPI 契约和两个仓库的 CHANGELOG。

## 8. 本地验证

启动后端：

```bash
cargo run
```

基础检查：

```bash
curl --fail http://127.0.0.1:8080/livez
curl --fail http://127.0.0.1:8080/readyz
curl http://127.0.0.1:8080/api/v1/api-docs/openapi.json
```

`/livez` 只证明进程存活并固定返回 `200`；后台任务定期检查 MySQL、required Redis 和必要对象存储，`/readyz` 只读取最近一次内存快照，请求路径不执行 SQL、Redis 或对象存储网络调用。必要依赖故障或快照过期时返回 `503`。探针不经过租户、认证、幂等和业务限流。

提交 API 变更前运行：

```bash
cargo run --locked -p ryframe-api --bin export_openapi -- openapi/openapi.json
cargo clippy --workspace --lib --bins -- -D warnings
```

架构和契约守卫会阻止漏写 OpenAPI 注解、漏注册文档、缺失成功响应 schema、缺失写请求体、查询参数覆盖回退、快照未同步、兼容路径别名和 Handler 直接访问数据库实现。
