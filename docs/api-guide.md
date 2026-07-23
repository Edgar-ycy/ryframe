# RyFrame API 使用指南

> 最后核对：2026-07-23
> API 版本：`v1`

本文档说明稳定约定和常见流程。所有路径、请求字段和响应 Schema 的唯一事实来源是 OpenAPI；运行时文档与仓库中的 `openapi/openapi.json` 必须精确一致：

```text
GET /api/v1/api-docs/openapi.json
GET /api/v1/swagger-ui
```

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
  "msg": "操作成功",
  "data": {}
}
```

无数据的成功响应可能省略或返回空 `data`，调用方应以 `code` 和 HTTP 状态判断结果。

### 分页响应

```json
{
  "code": 200,
  "msg": "查询成功",
  "rows": [],
  "total": 0
}
```

统一分页参数：

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `page` | integer | 页码，从 1 开始 |
| `page_size` | integer | 每页数量，受服务端上限约束 |

### ID

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
| `503` | Redis、数据库或必要对象存储暂不可用 |
| `500` | 未预期的服务端错误 |

客户端应展示服务端 `msg`，同时保留 HTTP 状态和请求 ID用于排障。

### 重试、幂等和代理边界

认证后的 `/system`、`/platform` 写请求可以携带 `Idempotency-Key`。服务端把它与租户、用户、HTTP 方法、规范路径和 body hash 绑定：处理中重复请求返回 `409` 与 `Retry-After`，完成结果保留 300 秒并可回放；同键不同 body 返回 `409`。超过 1 MiB 的成功响应不会被缓存，后续重复请求返回不可回放冲突，但首次成功响应保持不变。认证、上传下载、生成器、监控和流式响应不参与幂等缓存。

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

### 新密码策略

个人修改密码、密码重置完成和租户管理员初始密码使用同一策略：

- 长度为 8-72 个字符。
- 仅允许可见 ASCII 字符，不允许空格。
- 至少包含一个大写字母、一个小写字母、一个数字和一个特殊字符。

策略由 OpenAPI 顶层扩展 `x-ryframe-password-policy` 发布，各密码字段同时声明等价的 `minLength`、`maxLength` 和 `pattern`。前端必须从该扩展生成验证配置，不维护第二份正则。个人修改密码和重置密码成功后，服务端会递增用户 `auth_version`，此前签发的 access/refresh token 会失效。

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
| 全量列表 | `GET /resources/all` |
| 详情 | `GET /resources/{id}` |
| 创建 | `POST /resources` |
| 更新 | `PUT /resources/{id}` |
| 删除 | `DELETE /resources/{id}` |
| 领域动作 | `/resources/{id}/action` 或资源级动作路径 |

项目不保留旧接口别名。以下旧风格已禁止：`listNoPage`、`changeStatus`、`configKey`、`refreshCache` 和单数 `/system/user`、`/system/role`。

## 5. 模块目录

下表用于快速定位，具体字段和权限码查看 Swagger UI。

| 前缀 | 模块 | 额外动作 |
| --- | --- | --- |
| `/api/v1/system/users` | 用户 | `/all`、`PUT /{id}/roles`、`PUT /{id}/status`、`/batch/{ids}`、导入导出和重置请求 |
| `/api/v1/system/roles` | 角色 | `/all`、`GET/PUT /{id}/permissions`、`PUT /{id}/data-scope` |
| `/api/v1/system/perms` | 权限 | `/tree`、`/sync` |
| `/api/v1/system/menus` | 菜单 | `/tree`、`/current`、`/all` |
| `/api/v1/system/depts` | 部门 | `/tree`、`/all` |
| `/api/v1/system/posts` | 岗位 | `/all`、`/export` |
| `/api/v1/system/configs` | 参数配置 | `/all`、`/key/{key}`、`DELETE /cache`、`/export` |
| `/api/v1/system/dict` | 字典 | `/types`、`/types/all`、`/data`、`/data/type/{dict_type}` |
| `/api/v1/system/notices` | 通知公告 | `/all` |
| `/api/v1/system/operlogs` | 操作日志 | `/all`、`/export` |
| `/api/v1/system/loginlogs` | 登录日志 | `/all`、`/export` |
| `/api/v1/system/online` | 在线用户 | `/all`、`DELETE /{sid}`；`sid` 精确表示一个设备会话 |
| `/api/v1/platform/tenants` | 租户 | `PUT /{tenant_id}/status` |
| `/api/v1/auth/profile` | 个人中心 | `/password`、`/avatar` |
| `/api/v1/tools/gen` | 代码生成 | `/tables`、`/preview`、`/generate`、`/download` |
| `/api/v1/common/upload` | 文件上传 | `/image`、`/avatar` |
| `/api/v1/common/file` | 文件 | `/download` |
| `/api/v1/monitor` | 监控 | `/metrics`、`/server`、`/cache`、`/db-pool`、`/runtime`；探针位于根路径 `/livez`、`/readyz` |

### 当前用户菜单

登录后使用：

```text
GET /api/v1/system/menus/current
```

后端只返回稳定 `route_key`、菜单元数据和权限。前端必须通过本地页面注册表解析 `route_key`，不得执行服务端下发的任意组件路径。

### 角色分配

用户角色和角色权限均采用全量替换语义：

```text
PUT /api/v1/system/users/{id}/roles
PUT /api/v1/system/roles/{id}/permissions
PUT /api/v1/system/roles/{id}/data-scope
```

调用前先读取当前值，提交完整目标集合，不要只提交增量差异。创建用户时可直接提交 `role_ids`，用户和角色关联在同一数据库事务中创建；后续资料、角色和状态分别通过用户资源、`/{id}/roles` 和 `/{id}/status` 更新。数据范围请求同时提交 `data_scope` 和 `dept_ids`，两者在同一数据库事务中替换。

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

## 7. DTO 兼容规则

- 写入 DTO 默认拒绝未知字段；拼错字段会返回 `400`，不会被静默忽略。
- 状态、长度、邮箱、手机号和密码规则由服务端校验。
- 空字符串与 `null` 含义不同，调用方应按 OpenAPI Schema 发送。
- API v1 内进行破坏性重构时不保留旧路径，前后端必须在同一变更窗口更新。
- API 模块的字段或路径变更必须更新 OpenAPI 测试和两个仓库的 CHANGELOG。

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

`/livez` 只证明进程存活并固定返回 `200`；`/readyz` 检查 MySQL、required Redis 和必要对象存储，依赖故障时返回 `503`。探针不经过租户、认证、幂等和业务限流。

提交 API 变更前运行：

```bash
python scripts/check_architecture.py
cargo run -p ryframe-api --bin export_openapi -- openapi/openapi.json
cargo test -p ryframe-api
cargo clippy --workspace --all-targets -- -D warnings
```

架构和契约测试会阻止漏写 OpenAPI 注解、漏注册文档、缺失成功响应 schema、缺失写请求体、查询参数覆盖回退、快照未同步、兼容路径别名和 Handler 直接访问数据库实现。
