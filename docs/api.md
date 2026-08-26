# API

## 契约入口

HTTP 接口使用 `/api/v1` 前缀，完整路径、字段和 operation ID 见 `openapi/openapi.json`。开发环境启用 `api_docs.enabled` 并编译 `runtime-swagger-ui` feature 后，可访问 `/api/v1/swagger-ui`；原始文档位于 `/api/v1/api-docs/openapi.json`。

所有请求和响应使用 JSON，时间使用带时区的 RFC3339 并在服务端规范化为 UTC。DTO 默认拒绝未知字段；非法枚举、反向时间范围和越界批量请求在入队前返回 400。

普通响应使用统一包络：

```json
{
  "code": 200,
  "message": "操作成功",
  "data": {},
  "request_id": "01K...",
  "error_key": null,
  "details": null
}
```

## 认证与授权

先请求 `/api/v1/auth/csrf` 获取 CSRF token，登录、刷新和退出等会话写操作在 `X-CSRF-Token` 请求头中携带它。登录响应包含短期 access token，长期 refresh token 由 HttpOnly Cookie 保存。受保护接口使用：

```http
Authorization: Bearer <access_token>
```

会话上下文显式返回 `is_super_admin`。客户端使用该字段和授权投影展示界面，不根据角色 code 推断超级管理员。

每条路由在访问目录中声明一种策略：

- `Public`：无需登录。
- `Authenticated`：需要有效会话。
- `Permission`：需要生成的权限码。
- `Capability`：需要当前部署和租户同时启用能力。

前端缺权限进入 403，已知页面缺 capability 进入“功能不可用”，未知页面进入 404。

## 错误

错误响应包含稳定错误码、可展示信息和请求追踪信息。调用方应按 HTTP 状态和错误码处理，不解析中文文本。

- 400：请求或状态转换无效。
- 401：未认证或会话失效。
- 403：权限或租户能力不足。
- 404：资源不存在、越权或已删除。
- 409：资源状态冲突。
- 413：导出匹配行数或请求体超过限制。
- 501/503：部署能力不可用或依赖暂时不可用。

## 筛选导出

七类导出使用独立强类型请求，统一包络：

```json
{
  "filter": {},
  "confirm_all": false
}
```

请求不接受 `page` 或 `page_size`。导出选择页面最后一次成功应用的筛选，覆盖全部匹配分页；规范化后为空必须将 `confirm_all` 设为 `true`。

创建时同步完成匹配数、权限指纹和 `upper_id` 快照。Worker 只读取 `id <= upper_id`，后续新增不进入；权限发生任何变化均失败关闭。

稳定错误码包括：

- `EXPORT_ALL_CONFIRMATION_REQUIRED`
- `EXPORT_NO_MATCHING_ROWS`

## 导出记录删除

终态记录单删与批删共用：

```http
POST /api/v1/common/jobs/deletions
Idempotency-Key: <可重试键>
```

```json
{
  "ids": ["123", "456"]
}
```

ID 排序去重后必须为 1–100 条。整批先校验租户、申请人、终态和 lease，再标记删除；受理后立即从列表、未读和下载接口消失。对象删除失败会在内部重试，记录不会在网页复活。

## 契约验证

OpenAPI 改变后运行 `cargo api-sync` 刷新前端派生契约，再执行前端消费者自检与浏览器 smoke。若调用方提示 operation 不存在或 DTO 不匹配，先重新同步契约，再检查后端导出的 operation ID。
