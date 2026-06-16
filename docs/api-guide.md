# RyFrame API 开发指南

> **版本**: 0.5.0 | **基础路径**: `http://localhost:8080` | **API 前缀**: `/api/v1`

---

## 目录

- [1. 项目结构](#1-项目结构)
- [2. 路由约定](#2-路由约定)
- [3. 中间件执行顺序](#3-中间件执行顺序)
- [4. 认证流程](#4-认证流程)
- [5. 完整 API 路由表](#5-完整-api-路由表)
  - [5.1 认证](#51-认证)
  - [5.2 验证码](#52-验证码)
  - [5.3 个人中心](#53-个人中心)
  - [5.4 用户管理](#54-用户管理)
  - [5.5 角色管理](#55-角色管理)
  - [5.6 权限管理](#56-权限管理)
  - [5.7 菜单管理](#57-菜单管理)
  - [5.8 部门管理](#58-部门管理)
  - [5.9 岗位管理](#59-岗位管理)
  - [5.10 参数配置](#510-参数配置)
  - [5.11 字典管理](#511-字典管理)
  - [5.12 通知公告](#512-通知公告)
  - [5.13 操作日志](#513-操作日志)
  - [5.14 登录日志](#514-登录日志)
  - [5.15 定时任务](#515-定时任务)
  - [5.16 在线用户](#516-在线用户)
  - [5.17 监控](#517-监控)
  - [5.18 代码生成](#518-代码生成)
  - [5.19 通用](#519-通用)
- [6. 统一响应格式](#6-统一响应格式)
- [7. 分页约定](#7-分页约定)
- [8. 菜单系统设计](#8-菜单系统设计)
- [9. 数据权限体系](#9-数据权限体系)
- [10. 安全特性](#10-安全特性)
- [11. 操作日志](#11-操作日志)
- [12. 缓存使用](#12-缓存使用)
- [13. 消息队列](#13-消息队列)
- [14. 功能开关](#14-功能开关)
- [15. gRPC 通信](#15-grpc-通信)
- [16. 多租户](#16-多租户)
- [17. API 版本管理](#17-api-版本管理)

---

## 1. 项目结构

```
crates/
├── ryframe-api/         # API 层：路由、处理器、DTO、OpenAPI、操作日志中间件
│   ├── handlers/        # 19 个请求处理器
│   ├── dto/             # 15 个数据传输对象
│   ├── extractors/      # 自定义提取器
│   ├── router.rs        # 路由注册（auth/system/monitor/tools/common）
│   ├── openapi.rs       # utoipa OpenAPI 文档定义
│   ├── versioning.rs    # API 版本协商
│   └── oper_log_middleware.rs  # 操作日志记录
├── ryframe-service/     # 服务层：业务逻辑编排
├── ryframe-db/          # 数据层：实体、仓库、迁移
│   ├── entities/        # 19 个数据库实体
│   └── repositories/    # 14 个数据仓库
├── ryframe-auth/        # 认证授权：JWT、RBAC、密码哈希
├── ryframe-middleware/  # 15 个通用中间件
├── ryframe-core/        # 核心基础设施（缓存/消息队列/多租户/熔断器等）
├── ryframe-config/      # 配置加载与管理
├── ryframe-common/      # 公共工具库（错误类型、工具函数、枚举）
├── ryframe-monitor/     # 服务器监控
├── ryframe-task/        # 定时任务调度
├── ryframe-generator/   # 代码生成器
└── ryframe-macro/       # 派生宏（AutoFill 等）
```

**五层调用链**（以用户查询为例）：

```
Handler (user_handler::list)
  → Service (UserServiceImpl::find_page)
    → Repository (UserRepository::find_by_page)
      → SeaORM Entity (user::Entity::find)
        → MySQL / PostgreSQL / SQLite
```

---

## 2. 路由约定

| 约定 | 说明 |
|------|------|
| API 前缀 | 所有接口统一为 `/api/v1` |
| 公开路由 | `/api/v1/auth/login`、`/api/v1/auth/refresh`、`/api/v1/auth/captcha/**`、`/health`、`/metrics` |
| 受保护路由 | 需在请求头携带 `Authorization: Bearer <access_token>` |
| 文件上传 | 需认证，记录操作日志，上传成功后发布业务事件并投递后台任务 |
| 文件下载 | 需认证，记录操作日志 |
| OpenAPI JSON | `GET /api/v1/api-docs/openapi.json` |
| Swagger UI | `GET /api/v1/swagger-ui` |
| API 版本信息 | `GET /api/v1/version` |

---

## 3. 中间件执行顺序

### 全局层（`app.rs`）

后注册的 layer 为外层，先执行：

```
RateLimit(IP) → ApiRateLimit(per-endpoint) → Tenant → SecurityHeaders → Idempotency → ReplayProtection → CacheControl → BodyLimit(10MB) → Timeout(30s)
  → XssFilter → RequestLog → CORS → Compression → RequestId → Telemetry → Metrics
```

### 认证路由层（`auth_router`）

| 路由组 | 中间件执行顺序（外→内） | 说明 |
|--------|----------------------|------|
| public (`/login`, `/refresh`) | OperLog → Handler | 不认证，但记录操作者 = "anonymous" |
| protected (`/logout`, `/me`) | Auth → OperLog → Handler | 先校验 token，再记录操作日志 |
| profile (`/profile/*`) | Auth → OperLog → Handler | 同 protected |
| captcha (`/captcha/*`) | 无中间件 | 完全公开 |

### 系统管理路由层（`system_router`）

```
Auth → UserRateLimit → OnlineUserTracking → OperLog → Handler
```

### 工具路由层（`tools_router`）

```
Auth → UserRateLimit → OperLog → Handler
```

### 通用路由层（`common_router`）

| 路由组 | 中间件顺序 |
|--------|-----------|
| upload (`/upload/*`) | Auth → UserContext → OperLog → Handler |
| download (`/file/download`) | Auth → OperLog → Handler |

---

## 4. 认证流程

### 4.1 登录

```
POST /api/v1/auth/login
Content-Type: application/json

{
    "username": "admin",
    "password": "123456",
    "captcha_id": "uuid-from-captcha-generate",
    "captcha_code": "1234"
}
```

**响应**：

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "access_token": "eyJhbGciOiJIUzI1NiIs...",
        "refresh_token": "eyJhbGciOiJIUzI1NiIs...",
        "expires_in": 3600,
        "user_info": {
            "id": 1,
            "username": "admin",
            "nickname": "超级管理员",
            "email": "admin@ryframe.com",
            "roles": ["admin"],
            "perms": ["*:*:*"]
        }
    }
}
```

**access_token 内部结构**（Claims）：

| 字段 | 说明 |
|------|------|
| `sub` | 用户 ID（i64 字符串） |
| `username` | 用户名 |
| `roles` | 角色编码列表（嵌入 Claims，避免每次查库） |
| `perms` | 权限码列表（嵌入 Claims，避免每次查库） |
| `token_type` | `"access"` |
| `jti` | 令牌唯一 ID（雪花算法，用于在线用户管理） |
| `iat` | 签发时间 |
| `exp` | 过期时间 |

**默认账号**：

| 用户名 | 密码 | 角色 |
|--------|------|------|
| `admin` | `123456` | 超级管理员（全部权限） |
| `user` | `123456` | 普通用户（仅查看权限） |

**暴力破解防护**：连续失败超过配置阈值（默认 5 次）后，按用户名和 IP 两个维度分别锁定指定分钟数。

### 4.2 请求认证

所有受保护接口统一使用 Bearer Token：

```
Authorization: Bearer eyJhbGciOiJIUzI1NiIs...
```

认证中间件会：
1. 从请求头提取 token
2. 解码 JWT，验证签名和过期时间
3. 校验 `token_type == "access"`
4. 检查 token 是否在黑名单中（支持主动撤销）
5. 将 `Claims` 注入 `request.extensions()`，供后续中间件和 handler 使用

### 4.3 刷新令牌

access_token 过期后，使用 refresh_token 获取新的 access_token：

```
POST /api/v1/auth/refresh
Content-Type: application/json

{
    "refresh_token": "eyJhbGciOiJIUzI1NiIs..."
}
```

刷新时重新查询数据库获取最新角色/权限，确保权限变更即时生效。

### 4.4 登出

```
POST /api/v1/auth/logout
Authorization: Bearer {token}
```

登出后 token 被加入黑名单（Redis 或内存实现），直到 JWT 自然过期后自动清理。同时从在线用户列表中移除。

### 4.5 获取当前用户信息

```
GET /api/v1/auth/me
Authorization: Bearer {token}
```

返回当前用户的完整信息，包括角色列表和权限码列表。

---

## 5. 完整 API 路由表

### 5.1 认证

**基础路径**: `/api/v1/auth`

| 方法 | 路径 | 说明 | 认证 | 限流 |
|------|------|------|------|------|
| `POST` | `/login` | 用户登录 | 否 | IP 级别 |
| `POST` | `/refresh` | 刷新令牌 | 否 | - |
| `POST` | `/logout` | 用户登出 | 是 | - |
| `GET` | `/me` | 当前用户信息 | 是 | - |

### 5.2 验证码

**基础路径**: `/api/v1/auth/captcha`

| 方法 | 路径 | 说明 | 认证 |
|------|------|------|------|
| `GET` | `/generate` | 获取验证码（返回 Base64 JSON） | 否 |
| `GET` | `/image` | 获取验证码图片（PNG 二进制 + `X-Captcha-Id` 响应头） | 否 |
| `POST` | `/verify` | 校验验证码 | 否 |

**验证码生成响应**：

```json
{
    "code": 200,
    "data": {
        "captcha_id": "550e8400-e29b-41d4-a716-446655440000",
        "captcha_image": "data:image/png;base64,iVBORw0KGgo..."
    }
}
```

**验证码校验请求**：

```json
{
    "captcha_id": "550e8400-e29b-41d4-a716-446655440000",
    "captcha_code": "1234"
}
```

> **注意**：验证码可通过参数配置 `sys.account.captchaEnabled` 全局开关。验证码有效期 5 分钟，单次有效。

### 5.3 个人中心

**基础路径**: `/api/v1/auth/profile` | **全部需要认证**

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/` | 获取个人信息（含角色、权限、部门） |
| `PUT` | `/` | 更新个人信息（昵称、邮箱、手机号、性别） |
| `PUT` | `/password` | 修改密码（需提供旧密码） |
| `PUT` | `/avatar` | 更新头像 URL |

#### 5.3.1 获取个人信息

```
GET /api/v1/auth/profile
Authorization: Bearer {token}
```

**响应**：

```json
{
    "code": 200,
    "data": {
        "user_id": 1,
        "username": "admin",
        "nickname": "超级管理员",
        "email": "admin@ryframe.com",
        "phone": "13800000000",
        "avatar": null,
        "dept_id": 1,
        "dept_name": "RyFrame 科技",
        "status": "1",
        "login_ip": "127.0.0.1",
        "login_date": "2026-06-04T10:30:00",
        "created_at": "2026-05-22T00:00:00",
        "roles": ["admin"],
        "permissions": ["*:*:*"]
    }
}
```

**HTTP 状态码**: 200 | 401

#### 5.3.2 更新个人信息

```
PUT /api/v1/auth/profile
```

| 参数 | 类型 | 必填 | 验证 | 说明 |
|------|------|------|------|------|
| `nickname` | string | 是 | 长度 1-64 | 昵称 |
| `email` | string | 否 | 邮箱格式 | 邮箱 |
| `phone` | string | 否 | 正则 ^1[3-9]\d{9}$ | 手机号 |
| `sex` | string | 否 | - | 性别：0=男 1=女 2=未知 |

**请求示例**：

```json
{
    "nickname": "新昵称",
    "email": "new@example.com",
    "phone": "13800138000",
    "sex": "0"
}
```

**HTTP 状态码**: 200 | 400（参数校验失败）| 401

#### 5.3.3 修改密码

```
PUT /api/v1/auth/profile/password
```

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `old_password` | string | 是 | 旧密码，长度 6-100 |
| `new_password` | string | 是 | 新密码，长度 6-100，至少含字母和数字 |

**请求示例**：

```json
{
    "old_password": "123456",
    "new_password": "NewPass123"
}
```

**HTTP 状态码**: 200 | 400（旧密码错误或新密码不符合规范）| 401

#### 5.3.4 更新头像

```
PUT /api/v1/auth/profile/avatar
```

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `avatar_url` | string | 是 | 头像 URL 地址 |

**请求示例**：

```json
{
    "avatar_url": "/api/v1/common/file/download?path=2026/06/04/abc123.png"
}
```

**HTTP 状态码**: 200 | 400（URL为空）| 401

### 5.4 用户管理

**基础路径**: `/api/v1/system/users` | **全部需要认证 + 用户限流 + 操作日志**

| 方法 | 路径 | 说明 | 权限码 |
|------|------|------|--------|
| `GET` | `/list?page=1&pageSize=10&username=&status=` | 分页查询用户 | `system:user:list` |
| `GET` | `/listNoPage` | 用户列表（不分页，最多10000条） | `system:user:list` |
| `GET` | `/{id}` | 用户详情（含角色列表） | `system:user:list` |
| `POST` | `/` | 创建用户 | `system:user:add` |
| `PUT` | `/{id}` | 更新用户 | `system:user:edit` |
| `DELETE` | `/{id}` | 删除用户（软删除） | `system:user:remove` |
| `DELETE` | `/batch/{ids}` | 批量删除用户（逗号分隔ID） | `system:user:remove` |
| `PUT` | `/{id}/password` | 重置密码（管理员操作） | `system:user:edit` |
| `PUT` | `/changeStatus` | 修改状态（正常/停用/锁定） | `system:user:edit` |
| `GET` | `/export` | 导出用户数据为 Excel | `system:user:export` |
| `POST` | `/import` | 从 Excel 导入用户（multipart/form-data） | `system:user:import` |
| `GET` | `/import-template` | 下载导入模板 Excel | `system:user:import` |

#### 5.4.1 分页查询用户

```
GET /api/v1/system/users/list?page=1&pageSize=10&username=&phone=&status=&dept_id=
```

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `page` | u64 | 否 | 1 | 页码（从 1 开始） |
| `pageSize` | u64 | 否 | 10 | 每页条数 |
| `username` | string | 否 | - | 用户名（模糊搜索） |
| `phone` | string | 否 | - | 手机号（模糊搜索） |
| `dept_id` | i64 | 否 | - | 部门 ID 过滤 |
| `status` | string | 否 | - | 状态过滤：0=停用 1=正常 2=锁定 |

**成功响应**：

```json
{
    "code": 200,
    "msg": "查询成功",
    "data": {
        "rows": [{
            "id": 1,
            "username": "admin",
            "nickname": "超级管理员",
            "email": "admin@ryframe.com",
            "phone": "13800000000",
            "avatar": null,
            "dept_id": 1,
            "dept_name": "RyFrame 科技",
            "status": "1",
            "remark": null,
            "created_at": "2026-05-22T00:00:00"
        }],
        "total": 1
    }
}
```

**HTTP 状态码**: 200 | 401（未认证）| 403（无权限）

#### 5.4.2 用户详情

```
GET /api/v1/system/users/{id}
```

**响应**：

```json
{
    "code": 200,
    "data": {
        "id": 1,
        "username": "admin",
        "nickname": "超级管理员",
        "email": "admin@ryframe.com",
        "phone": "13800000000",
        "dept_id": 1,
        "dept_name": "RyFrame 科技",
        "status": "1",
        "role_ids": [1],
        "role_names": ["admin"],
        "created_at": "2026-05-22T00:00:00",
        "updated_at": "2026-06-01T00:00:00"
    }
}
```

**HTTP 状态码**: 200 | 401 | 403 | 404（用户不存在）

#### 5.4.3 创建用户

```
POST /api/v1/system/users
```

| 参数 | 类型 | 必填 | 验证 | 说明 |
|------|------|------|------|------|
| `username` | string | 是 | 长度 1-50 | 用户名（唯一） |
| `password` | string | 是 | 长度 6-100 | 密码（可启用复杂度校验） |
| `nickname` | string | 是 | 长度 ≥1 | 用户昵称 |
| `email` | string | 否 | - | 邮箱 |
| `phone` | string | 否 | - | 手机号 |
| `dept_id` | i64 | 否 | - | 所属部门 ID |
| `role_ids` | i64[] | 否 | - | 角色 ID 列表 |

**请求示例**：

```json
{
    "username": "new_user",
    "password": "123456",
    "nickname": "新用户",
    "email": "user@example.com",
    "phone": "13900000000",
    "dept_id": 5,
    "role_ids": [2]
}
```

**HTTP 状态码**: 200 | 400（参数校验失败）| 401 | 403 | 409（用户名已存在）

#### 5.4.4 更新用户

```
PUT /api/v1/system/users/{id}
```

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `nickname` | string | 是 | 用户昵称 |
| `email` | string | 否 | 邮箱 |
| `phone` | string | 否 | 手机号 |
| `dept_id` | i64 | 否 | 部门 ID |
| `status` | string | 是 | 状态：0/1/2 |
| `role_ids` | i64[] | 否 | 角色 ID 列表 |

**请求示例**：

```json
{
    "nickname": "新昵称",
    "email": "new@example.com",
    "phone": "13800000001",
    "dept_id": 3,
    "status": "1",
    "role_ids": [1, 2]
}
```

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

#### 5.4.5 删除用户（软删除）

```
DELETE /api/v1/system/users/{id}
```

**HTTP 状态码**: 200 | 401 | 403 | 404

#### 5.4.6 批量删除用户

```
DELETE /api/v1/system/users/batch/1,2,3
```

路径参数为逗号分隔的用户 ID 列表。

**失败响应（ID 列表为空）**：

```json
{ "code": 400, "message": "请选择要删除的用户", "data": null }
```

**HTTP 状态码**: 200 | 400（ID列表为空）| 401 | 403

#### 5.4.7 重置密码

```
PUT /api/v1/system/users/{id}/password
```

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `password` | string | 是 | 长度 6-100，新密码 |

**请求示例**：

```json
{
    "password": "NewPass123"
}
```

**HTTP 状态码**: 200 | 400（密码不符合规范）| 401 | 403 | 404

#### 5.4.8 修改用户状态

```
PUT /api/v1/system/users/changeStatus
```

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `user_id` | i64 | 是 | 用户 ID |
| `status` | string | 是 | 状态：0=停用 1=正常 2=锁定 |

**请求示例**：

```json
{
    "user_id": 3,
    "status": "0"
}
```

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

#### 5.4.9 导出用户（Excel）

```
GET /api/v1/system/users/export
```

返回 `.xlsx` 二进制文件（`Content-Type: application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`）。

导出字段：user_id, username, nickname, email, phone, sex, dept_name, status, remark, created_at。

**HTTP 状态码**: 200 | 401 | 403

#### 5.4.10 导入用户（Excel）

```
POST /api/v1/system/users/import
Content-Type: multipart/form-data
```

上传字段名：`file`（.xlsx 文件）。导入模板列：username, nickname, email, phone, sex, dept_id, status, remark。

**响应示例**：

```json
{
    "code": 200,
    "message": "导入完成",
    "data": {
        "success_count": 5,
        "fail_count": 1,
        "errors": ["第 3 行数据验证失败: 用户名长度为2-64个字符"]
    }
}
```

**HTTP 状态码**: 200 | 400（未找到上传文件）| 401 | 403

#### 5.4.11 下载导入模板

```
GET /api/v1/system/users/import-template
```

返回 `.xlsx` 模板文件（包含表头，无数据行），文件名 `user_template.xlsx`。

**HTTP 状态码**: 200 | 401 | 403

**用户状态值**：

| 值 | 含义 |
|----|------|
| `1` | 正常 |
| `0` | 停用 |
| `2` | 锁定 |

### 5.5 角色管理

**基础路径**: `/api/v1/system/roles` | **全部需要认证 + 用户限流 + 操作日志**

| 方法 | 路径 | 说明 | 权限码 |
|------|------|------|--------|
| `GET` | `/list?page=1&pageSize=10&name=&code=&status=` | 角色列表（分页） | `system:role:list` |
| `GET` | `/listNoPage` | 角色列表（不分页） | `system:role:list` |
| `GET` | `/export` | 导出角色 Excel | `system:role:export` |
| `GET` | `/{id}` | 角色详情 | `system:role:list` |
| `POST` | `/` | 创建角色 | `system:role:add` |
| `PUT` | `/{id}` | 更新角色 | `system:role:edit` |
| `DELETE` | `/{id}` | 删除角色 | `system:role:remove` |
| `DELETE` | `/batch/{ids}` | 批量删除（逗号分隔ID） | `system:role:remove` |
| `PUT` | `/{id}/permissions` | 分配权限（role_permission） | `system:role:edit` |
| `PUT` | `/{id}/menus` | 分配菜单（role_menu） | `system:role:edit` |
| `PUT` | `/{id}/data-scope` | 设置数据权限 | `system:role:edit` |

#### 5.5.1 分页查询角色

```
GET /api/v1/system/roles/list?page=1&pageSize=10&name=&code=&status=
```

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `page` | u64 | 否 | 1 | 页码 |
| `pageSize` | u64 | 否 | 10 | 每页条数 |
| `name` | string | 否 | - | 角色名称（模糊搜索） |
| `code` | string | 否 | - | 角色编码（模糊搜索） |
| `status` | string | 否 | - | 状态过滤：0/1 |

**成功响应**：

```json
{
    "code": 200,
    "msg": "查询成功",
    "data": {
        "rows": [{
            "id": 1,
            "name": "超级管理员",
            "code": "admin",
            "data_scope": "1",
            "sort": 1,
            "status": "1",
            "remark": null,
            "created_at": "2026-05-22T00:00:00"
        }],
        "total": 1
    }
}
```

**HTTP 状态码**: 200 | 401 | 403

#### 5.5.2 创建角色

```
POST /api/v1/system/roles
```

| 参数 | 类型 | 必填 | 验证 | 说明 |
|------|------|------|------|------|
| `name` | string | 是 | 长度 1-50 | 角色名称 |
| `code` | string | 是 | 长度 1-50 | 角色编码（唯一） |
| `sort` | i32 | 否 | - | 显示顺序，默认 0 |
| `data_scope` | string | 否 | - | 数据范围：1/2/3/4/5，默认 1 |

**请求示例**：

```json
{
    "name": "测试角色",
    "code": "test_role",
    "sort": 3,
    "data_scope": "5"
}
```

**HTTP 状态码**: 200 | 400（参数校验失败）| 401 | 403 | 409（code 重复）

#### 5.5.3 更新角色

```
PUT /api/v1/system/roles/{id}
```

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | 角色名称 |
| `sort` | i32 | 否 | 排序值 |
| `status` | string | 是 | 状态：0/1 |
| `data_scope` | string | 否 | 数据范围 |

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

#### 5.5.4 删除角色

```
DELETE /api/v1/system/roles/{id}
```
**HTTP 状态码**: 200 | 401 | 403 | 404

#### 5.5.5 批量删除角色

```
DELETE /api/v1/system/roles/batch/1,2,3
```

**HTTP 状态码**: 200 | 400（ID列表为空）| 401 | 403

#### 5.5.6 分配权限

```
PUT /api/v1/system/roles/{id}/permissions
```

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `perm_ids` | i64[] | 是 | 权限 ID 列表（全量覆盖） |

**请求示例**：

```json
{
    "perm_ids": [7, 8, 9, 10]
}
```

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

#### 5.5.7 分配菜单

```
PUT /api/v1/system/roles/{id}/menus
```

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `menu_ids` | i64[] | 是 | 菜单 ID 列表（全量覆盖） |

**请求示例**：

```json
{
    "menu_ids": [0, 1, 4]
}
```

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

#### 5.5.8 设置数据权限

```
PUT /api/v1/system/roles/{id}/data-scope
```

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `data_scope` | string | 是 | 数据范围：1/2/3/4/5 |
| `dept_ids` | i64[] | 否 | 自定义部门 ID 列表（仅 data_scope=2 时有效） |

**请求示例（自定义部门）**：

```json
{
    "data_scope": "2",
    "dept_ids": [1, 5, 10]
}
```

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

#### 5.5.9 导出角色

```
GET /api/v1/system/roles/export
```

返回 `.xlsx` 文件，导出字段：role_id, role_name, role_code, data_scope, status, sort, remark, created_at。

**HTTP 状态码**: 200 | 401 | 403

**数据权限 (data_scope)**：

| 值 | 含义 | 说明 |
|----|------|------|
| `1` | 全部数据 | 可查看所有部门数据 |
| `2` | 自定义 | 通过 `sys_role_dept` 关联指定部门 |
| `3` | 本部门 | 仅可查看本部门数据 |
| `4` | 本部门及以下 | 可查看本部门及所有子部门 |
| `5` | 仅本人 | 只能查看自己的数据 |

### 5.6 权限管理

**基础路径**: `/api/v1/system/permissions` | **需要认证**

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/tree` | 权限树（树形结构） |

权限码存储在 `sys_permission` 表中，分为两类：
- `perm_type = "menu"`：菜单级权限（侧边栏可见性）
- `perm_type = "api"`：API 级权限（按钮/接口访问控制）

### 5.7 菜单管理

**基础路径**: `/api/v1/system/menus` | **全部需要认证 + 用户限流 + 操作日志**

| 方法 | 路径 | 说明 | 权限码 |
|------|------|------|--------|
| `GET` | `/tree` | 菜单树（含目录 M/菜单 C/按钮 F） | `system:menu:list` |
| `GET` | `/list?page=1&pageSize=10&name=&status=` | 菜单列表（分页） | `system:menu:list` |
| `GET` | `/listNoPage` | 菜单列表（不分页） | `system:menu:list` |
| `GET` | `/{id}` | 菜单详情 | `system:menu:list` |
| `POST` | `/` | 创建菜单 | `system:menu:add` |
| `PUT` | `/{id}` | 更新菜单 | `system:menu:edit` |
| `DELETE` | `/{id}` | 删除菜单（软删除） | `system:menu:remove` |

**分页查询参数**：

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `page` | u64 | 否 | 1 | 页码 |
| `pageSize` | u64 | 否 | 10 | 每页条数 |
| `name` | string | 否 | - | 菜单名称（模糊搜索） |
| `status` | string | 否 | - | 状态：0/1 |

**菜单 DTO 详细字段**：

| 字段 | 类型 | 创建 | 更新 | 说明 |
|------|------|------|------|------|
| `name` | string | 必填 | 必填 | 菜单名称 |
| `parent_id` | i64/null | 选填 | 选填 | 父菜单 ID，顶级为 null |
| `menu_type` | string | 必填 | 必填 | `M`=目录 `C`=菜单 `F`=按钮 |
| `path` | string/null | 选填 | 选填 | 路由路径（目录/菜单） |
| `component` | string/null | 选填 | 选填 | 前端组件路径（仅菜单类型） |
| `query` | string/null | 选填 | 选填 | 路由参数，如 `id=1` |
| `perms` | string/null | 选填 | 选填 | 权限标识，如 `system:user:list` |
| `icon` | string/null | 选填 | 选填 | 图标名称 |
| `is_frame` | bool | 选填 | 选填 | 是否外链（0=否 1=是） |
| `is_cache` | bool | 选填 | 选填 | 是否缓存页面（0=否 1=是） |
| `sort` | i32 | 选填 | 选填 | 显示顺序，默认 0 |
| `visible` | bool | 选填 | 选填 | 是否可见，默认 true |
| `status` | string | 自动 | 必填 | `1`=正常 `0`=停用 |

**创建目录示例**：

```json
{
    "name": "内容管理",
    "menu_type": "M",
    "parent_id": null,
    "path": "/content",
    "icon": "Document",
    "sort": 4,
    "visible": true
}
```

**创建菜单示例**：

```json
{
    "name": "文章管理",
    "menu_type": "C",
    "parent_id": 23,
    "path": "/content/article",
    "component": "content/article/index",
    "perms": "content:article:list",
    "icon": "Notebook",
    "sort": 1,
    "is_cache": true,
    "visible": true
}
```

**创建按钮示例**：

```json
{
    "name": "文章新增",
    "menu_type": "F",
    "parent_id": 24,
    "perms": "content:article:add",
    "sort": 1,
    "visible": true
}
```

**默认菜单树结构**（种子数据）：

```
首页 (C) ── /dashboard
系统管理 (M)
  ├── 用户管理 (C)
  │   ├── 用户查询 (F) ─── system:user:list
  │   ├── 用户新增 (F) ─── system:user:add
  │   ├── 用户修改 (F) ─── system:user:edit
  │   ├── 用户删除 (F) ─── system:user:remove
  │   └── 用户导出 (F) ─── system:user:export
  ├── 角色管理 (C)
  ├── 菜单管理 (C)
  ├── 部门管理 (C)
  ├── 岗位管理 (C)
  ├── 字典管理 (C)
  ├── 参数设置 (C)
  ├── 通知公告 (C)
  ├── 操作日志 (C)
  ├── 登录日志 (C)
  └── 定时任务 (C)
系统监控 (M)
  ├── 在线用户 (C)
  └── 服务监控 (C)
系统工具 (M)
  └── 代码生成 (C)
```

### 5.8 部门管理

**基础路径**: `/api/v1/system/depts` | **全部需要认证 + 操作日志**

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/tree` | 部门树（含 ancestors 祖级列表） |
| `GET` | `/list?page=1&pageSize=10&name=&status=` | 部门列表（分页） |
| `GET` | `/listNoPage` | 部门列表（不分页） |
| `GET` | `/{id}` | 部门详情 |
| `POST` | `/` | 创建部门 |
| `PUT` | `/{id}` | 更新部门 |
| `DELETE` | `/{id}` | 删除部门 |

**分页查询参数**：

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `page` | u64 | 否 | 1 | 页码 |
| `pageSize` | u64 | 否 | 10 | 每页条数 |
| `name` | string | 否 | - | 部门名称（模糊搜索） |
| `status` | string | 否 | - | 状态过滤：0/1 |

**创建部门**：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | 长度 ≥1，部门名称 |
| `parent_id` | i64 | 否 | 父部门 ID |
| `sort` | i32 | 否 | 显示顺序，默认 0 |

**请求示例**：

```json
{
    "name": "技术部",
    "parent_id": 1,
    "sort": 2
}
```

**更新部门**：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | 部门名称 |
| `parent_id` | i64 | 否 | 父部门 ID |
| `sort` | i32 | 否 | 排序值 |
| `status` | string | 是 | 状态：0/1 |

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

### 5.9 岗位管理

**基础路径**: `/api/v1/system/posts` | **全部需要认证 + 操作日志**

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list?page=1&pageSize=10&name=&code=&status=` | 岗位列表（分页） |
| `GET` | `/listNoPage` | 岗位列表（不分页） |
| `GET` | `/export` | 导出岗位 Excel |
| `GET` | `/{id}` | 岗位详情 |
| `POST` | `/` | 创建岗位 |
| `PUT` | `/{id}` | 更新岗位 |
| `DELETE` | `/{id}` | 删除岗位 |

**分页查询参数**：

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `page` | u64 | 否 | 1 | 页码 |
| `pageSize` | u64 | 否 | 10 | 每页条数 |
| `name` | string | 否 | - | 岗位名称（模糊搜索） |
| `code` | string | 否 | - | 岗位编码（模糊搜索） |
| `status` | string | 否 | - | 状态过滤：0/1 |

**创建岗位**：

| 参数 | 类型 | 必填 | 验证 | 说明 |
|------|------|------|------|------|
| `name` | string | 是 | 长度 ≥1 | 岗位名称 |
| `code` | string | 是 | 长度 ≥1 | 岗位编码 |
| `sort` | i32 | 否 | - | 显示顺序 |

**请求示例**：

```json
{
    "name": "董事长",
    "code": "chairman",
    "sort": 1
}
```

**更新岗位**：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | 岗位名称 |
| `sort` | i32 | 否 | 排序值 |
| `status` | string | 是 | 状态：0/1 |

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

**导出岗位**：`GET /api/v1/system/posts/export` 返回 `.xlsx`，字段：post_id, name, code, sort, status, remark, created_at。

### 5.10 参数配置

**基础路径**: `/api/v1/system/configs` | **全部需要认证 + 操作日志**

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list?page=1&pageSize=10` | 配置列表（分页） |
| `GET` | `/listNoPage` | 配置列表（不分页） |
| `GET` | `/export` | 导出配置 Excel |
| `GET` | `/{id}` | 配置详情 |
| `GET` | `/configKey/{key}` | 按 Key 精确查询（只返回值） |
| `POST` | `/` | 创建配置 |
| `PUT` | `/{id}` | 更新配置 |
| `DELETE` | `/{id}` | 删除配置 |
| `DELETE` | `/refreshCache` | 刷新参数缓存（清空 Redis 中 sys_config:key:*） |

**创建配置**：

| 参数 | 类型 | 必填 | 验证 | 说明 |
|------|------|------|------|------|
| `name` | string | 是 | 长度 1-100 | 参数名称 |
| `key` | string | 是 | 长度 1-100 | 参数键名（唯一） |
| `value` | string | 是 | 长度 1-500 | 参数键值 |
| `remark` | string | 否 | - | 备注 |

**请求示例**：

```json
{
    "name": "文件上传大小限制",
    "key": "sys.upload.fileSize",
    "value": "10485760",
    "remark": "单位：字节"
}
```

**更新配置**：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `value` | string | 是 | 参数键值（长度≥1） |

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

**按 Key 查询**：`GET /api/v1/system/configs/configKey/sys.account.captchaEnabled` → 直接返回 `{"code":200,"data":"true"}`。

**导出配置**：`GET /api/v1/system/configs/export` → `.xlsx`，字段：name, key, value, remark, created_at。

**默认配置项**：

| Key | 值 | 说明 |
|-----|-----|------|
| `sys.index.skinName` | `skin-blue` | 默认皮肤样式 |
| `sys.user.initPassword` | `123456` | 新增用户初始密码 |
| `sys.index.sideTheme` | `theme-dark` | 侧边栏主题 |
| `sys.account.captchaEnabled` | `true` | 验证码开关 |

### 5.11 字典管理

**基础路径**: `/api/v1/system/dict` | **需要认证 + 操作日志**

**字典类型**：

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/types/list?page=1&pageSize=10` | 字典类型列表（分页） |
| `GET` | `/types/listNoPage` | 字典类型列表（不分页） |
| `GET` | `/types/export` | 导出字典类型 Excel |
| `POST` | `/types` | 创建字典类型 |
| `PUT` | `/types/{id}` | 更新字典类型 |
| `DELETE` | `/types/{id}` | 删除字典类型 |

**字典数据**：

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/data?type_code=xxx` | 按类型编码查询字典数据列表 |
| `GET` | `/data/type/{dict_type}` | 按路径参数查询（返回 dictLabel/dictValue/cssClass） |
| `POST` | `/data` | 创建字典数据 |
| `PUT` | `/data/{id}` | 更新字典数据 |
| `DELETE` | `/data/{id}` | 删除字典数据 |

**创建字典类型**：

| 参数 | 类型 | 必填 | 验证 | 说明 |
|------|------|------|------|------|
| `name` | string | 是 | 长度 ≥1 | 字典名称 |
| `code` | string | 是 | 长度 ≥1 | 字典编码（唯一） |

**创建字典数据**：

| 参数 | 类型 | 必填 | 验证 | 说明 |
|------|------|------|------|------|
| `type_code` | string | 是 | - | 所属字典类型编码 |
| `label` | string | 是 | 长度 ≥1 | 字典标签（显示名） |
| `value` | string | 是 | 长度 ≥1 | 字典值 |
| `sort` | i32 | 否 | - | 排序，默认 0 |

**请求示例（创建字典数据）**：

```json
{
    "type_code": "sys_user_sex",
    "label": "男",
    "value": "0",
    "sort": 0
}
```

**字典数据路径查询** `GET /api/v1/system/dict/data/type/sys_user_sex`：

```json
{
    "code": 200,
    "data": [
        { "dictLabel": "男", "dictValue": "0", "cssClass": null },
        { "dictLabel": "女", "dictValue": "1", "cssClass": null },
        { "dictLabel": "未知", "dictValue": "2", "cssClass": null }
    ]
}
```

**更新字典数据**：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `label` | string | 是 | 字典标签 |
| `value` | string | 是 | 字典值 |
| `sort` | i32 | 否 | 排序值 |
| `status` | string | 是 | 状态：0/1 |

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

**导出字典类型**：`GET /api/v1/system/dict/types/export` → `.xlsx`，字段：name, code, status, remark, created_at。

### 5.12 通知公告

**基础路径**: `/api/v1/system/notices` | **需要认证 + 操作日志**

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list?page=1&pageSize=10&title=&notice_type=&status=` | 通知列表（分页） |
| `GET` | `/listNoPage` | 通知列表（不分页） |
| `GET` | `/{id}` | 通知详情 |
| `POST` | `/` | 创建通知 |
| `PUT` | `/{id}` | 更新通知 |
| `DELETE` | `/{id}` | 删除通知 |

**分页查询参数**：

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `page` | u64 | 否 | 1 | 页码 |
| `pageSize` | u64 | 否 | 10 | 每页条数 |
| `title` | string | 否 | - | 标题（模糊搜索） |
| `notice_type` | string | 否 | - | 类型：notice/announcement |
| `status` | string | 否 | - | 状态：0/1/2 |

**创建通知**：

| 参数 | 类型 | 必填 | 验证 | 说明 |
|------|------|------|------|------|
| `title` | string | 是 | 长度 ≥1 | 标题 |
| `content` | string | 是 | 长度 ≥1 | 内容 |
| `notice_type` | string | 否 | - | 类型，默认 notice |

**请求示例**：

```json
{
    "title": "系统维护通知",
    "content": "系统将于本周六凌晨进行维护",
    "notice_type": "notice"
}
```

**更新通知**：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `title` | string | 是 | 标题 |
| `content` | string | 是 | 内容 |
| `notice_type` | string | 否 | 类型 |
| `status` | string | 是 | 状态：0/1/2 |

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

**通知状态**：`0`=草稿 `1`=已发布 `2`=已关闭

**通知类型**：`notice`=通知 `announcement`=公告

### 5.13 操作日志

**基础路径**: `/api/v1/system/operlogs` | **需要认证**

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list?page=1&pageSize=10&oper_name=&status=&begin_time=&end_time=` | 操作日志分页查询 |
| `GET` | `/listNoPage` | 操作日志不分页查询 |
| `GET` | `/export` | 导出操作日志 Excel |
| `DELETE` | `/clean` | 清空全部操作日志 |

**查询参数**：

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `page` | u64 | 否 | 1 | 页码 |
| `pageSize` | u64 | 否 | 10 | 每页条数 |
| `oper_name` | string | 否 | - | 操作人（模糊） |
| `status` | string | 否 | - | 状态：0=失败 1=成功 |
| `begin_time` | string | 否 | - | 开始时间，格式 yyyy-MM-dd HH:mm:ss |
| `end_time` | string | 否 | - | 结束时间 |

**导出**：`GET /api/v1/system/operlogs/export` → `.xlsx`，字段：title, business_type, oper_name, oper_url, oper_ip, status, cost_time, oper_time。

**日志包含信息**：操作人、IP、URL、请求方法、请求参数、响应结果、耗时(ms)、业务类型。

### 5.14 登录日志

**基础路径**: `/api/v1/system/loginlogs` | **需要认证**

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list?page=1&pageSize=10&user_name=&status=&begin_time=&end_time=` | 登录日志分页查询 |
| `GET` | `/listNoPage` | 登录日志不分页查询 |
| `GET` | `/export` | 导出登录日志 Excel |
| `DELETE` | `/clean` | 清空全部登录日志 |

**查询参数**：

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `page` | u64 | 否 | 1 | 页码 |
| `pageSize` | u64 | 否 | 10 | 每页条数 |
| `user_name` | string | 否 | - | 用户名（模糊） |
| `status` | string | 否 | - | 状态：0=失败 1=成功 |
| `begin_time` | string | 否 | - | 开始时间 |
| `end_time` | string | 否 | - | 结束时间 |

**导出**：`GET /api/v1/system/loginlogs/export` → `.xlsx`，字段：user_name, ipaddr, login_location, browser, os, status, msg, login_time。

**日志包含信息**：用户名、IP、地址、浏览器、操作系统、登录状态(0=失败/1=成功)、提示信息、登录时间。

### 5.15 定时任务

**基础路径**: `/api/v1/system/jobs` | **需要认证 + 操作日志**

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list?page=1&pageSize=10` | 任务列表（分页） |
| `GET` | `/listNoPage` | 任务列表（不分页） |
| `POST` | `/` | 创建任务 |
| `PUT` | `/{id}` | 更新任务 |
| `DELETE` | `/{id}` | 删除任务 |
| `POST` | `/{id}/pause` | 暂停任务 |
| `POST` | `/{id}/resume` | 恢复任务 |
| `POST` | `/{id}/trigger` | 立即执行一次 |
| `GET` | `/logs?page=1&pageSize=10&job_name=&status=` | 任务执行日志 |

**创建任务**：

| 参数 | 类型 | 必填 | 验证 | 说明 |
|------|------|------|------|------|
| `name` | string | 是 | 长度 1-100 | 任务名称 |
| `cron_expr` | string | 是 | 长度 1-100 | Cron 表达式（如 `0 0 3 * * *`） |
| `group_name` | string | 否 | - | 任务分组名 |
| `misfire_policy` | string | 否 | - | 错过策略：1=立即执行 2=放弃 |
| `concurrent` | string | 否 | - | 是否并发：0=禁止 1=允许 |
| `remark` | string | 否 | - | 备注 |

**请求示例**：

```json
{
    "name": "clean_temp_files",
    "cron_expr": "0 0 3 * * *",
    "group_name": "system",
    "misfire_policy": "1",
    "concurrent": "0",
    "remark": "每天凌晨 3 点清理临时文件"
}
```

**更新任务**：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `cron_expr` | string | 否 | Cron 表达式 |
| `status` | string | 否 | 状态 |
| `remark` | string | 否 | 备注 |

**HTTP 状态码**: 200 | 400 | 401 | 403 | 404

**任务执行日志查询参数**：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |
| `job_name` | string | 否 | 任务名称 |
| `status` | string | 否 | 执行状态 |

**立即触发**：`POST /api/v1/system/jobs/{id}/trigger` 返回 `TaskHistory` 对象。

### 5.16 在线用户

**基础路径**: `/api/v1/system/online` | **需要认证**

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list?page=1&pageSize=10&username=&ipaddr=` | 在线用户列表（分页） |
| `GET` | `/listNoPage` | 在线用户列表（不分页） |
| `DELETE` | `/{token_id}` | 强制下线（token 加入黑名单） |

**查询参数**：

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `page` | u64 | 否 | 1 | 页码 |
| `pageSize` | u64 | 否 | 10 | 每页条数 |
| `username` | string | 否 | - | 用户名（模糊搜索） |
| `ipaddr` | string | 否 | - | IP 地址（模糊搜索） |

**在线用户信息**：token_id, user_id, username, dept_name, ipaddr, login_location, browser, os, login_time, last_access_time。

**HTTP 状态码**: 200 | 400 | 401 | 403

### 5.17 监控

**基础路径**: `/api/v1/monitor`

| 方法 | 路径 | 说明 | 认证 |
|------|------|------|------|
| `GET` | `/health` | 健康检查（DB + Redis 连通性）+ 运行时间 | 否 |
| `GET` | `/metrics` | Prometheus 指标（text/plain 格式） | 否 |
| `GET` | `/server` | 服务器信息：CPU 使用率、内存、磁盘、系统信息 | 是 |
| `GET` | `/cache` | Redis 缓存命中率、内存占用、键数量 | 是 |
| `GET` | `/cache/commands` | Redis 命令统计 | 是 |
| `GET` | `/db-pool` | 数据库连接池活跃/空闲连接数 | 是 |

**健康检查响应示例**：

```json
{
    "status": "UP",
    "version": "0.5.0",
    "uptime_seconds": 86400,
    "checks": {
        "database": "UP",
        "redis": "UP"
    }
}
```

### 5.18 代码生成

**基础路径**: `/api/v1/tools/gen` | **需要认证 + 用户限流**

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/tables` | 数据库表列表 |
| `POST` | `/preview` | 预览生成代码 |
| `POST` | `/generate` | 执行代码生成（写入磁盘） |
| `POST` | `/download` | 打包 zip 下载 |

**生成选项 (GenerateOptions)**：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `table_name` | string | 是 | 数据库表名（如 sys_user） |
| `module_name` | string | 否 | 模块名称 |
| `business_name` | string | 否 | 业务名称 |
| `class_name` | string | 否 | 类名 |
| `package_name` | string | 否 | 包名 |
| `author` | string | 否 | 作者 |

**请求示例**：

```json
{
    "table_name": "sys_user",
    "module_name": "system",
    "business_name": "user",
    "class_name": "User"
}
```

生成产物：Entity → Repository → Service → Handler → DTO（五层架构）。

**HTTP 状态码**: 200 | 400 | 401 | 403

### 5.19 通用

**基础路径**: `/api/v1/common`

| 方法 | 路径 | 说明 | 认证 | 操作日志 |
|------|------|------|------|----------|
| `POST` | `/upload` | 文件上传（multipart/form-data） | 是 | 记录 |
| `POST` | `/upload/image` | 图片上传（自动压缩，限 jpg/png/gif/bmp/webp） | 是 | 记录 |
| `GET` | `/file/download?path=...&bucket=...` | 文件下载 | 是 | 记录 |

---

## 6. 统一响应格式

所有 API 返回统一的 JSON 结构：

**成功响应**：

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": { ... }
}
```

**分页成功响应**：

```json
{
    "code": 200,
    "msg": "查询成功",
    "rows": [...],
    "total": 100
}
```

**错误响应**：

```json
{
    "code": 400,
    "msg": "用户名不能为空",
    "data": null
}
```

**HTTP 状态码与业务 Code 对照**：

| HTTP 状态码 | 业务含义 | 触发场景 |
|-------------|----------|----------|
| 200 | 操作成功 | 正常的 CRUD 操作 |
| 400 | 请求参数错误 | 字段校验失败、缺少必填参数 |
| 401 | 未认证 | token 无效、过期、被撤销、或未携带 token |
| 403 | 无权限 | 缺少所需权限码、租户信息缺失 |
| 404 | 资源不存在 | 查询/更新/删除不存在的记录 |
| 409 | 数据冲突 | 唯一键重复、幂等性键冲突 |
| 429 | 请求过于频繁 | 触发限流（IP 级或用户级） |
| 500 | 服务器内部错误 | 数据库错误、未预期的 panic |

---

## 7. 分页约定

**请求参数**：

```
GET /api/v1/system/users/list?page=1&pageSize=10&sort_field=id&sort_order=desc
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `page` | u64 | 1 | 页码（从 1 开始） |
| `pageSize` | u64 | 10 | 每页条数 |
| `sort_field` | string | `id` | 排序字段 |
| `sort_order` | string | `desc` | 排序方向：`asc` / `desc` |

**响应格式**：

```json
{
    "code": 200,
    "msg": "查询成功",
    "data": {
        "rows": [
            { "id": 1, "username": "admin", "nickname": "超级管理员", ... }
        ],
        "total": 1
    }
}
```

---

## 8. 菜单系统设计

RyFrame 的菜单系统将**目录**、**菜单页面**、**操作按钮**统一存储在 `sys_menu` 表中，通过 `menu_type` 字段区分。

### 8.1 三种节点类型

| 类型 | `menu_type` | 前端渲染 | 路由 | 权限控制 |
|------|-------------|----------|------|----------|
| 目录 | `M` | 侧边栏分组标题（不可点击） | 有 `path`，无实际页面 | 无 |
| 菜单 | `C` | 侧边栏菜单项（可点击导航） | 有 `path` + `component` | 通过 `perms` 控制可见性 |
| 按钮 | `F` | 页面内操作按钮（新增/编辑/删除等） | 无路由 | 通过 `perms` 控制显示/隐藏 |

### 8.2 菜单树接口

`GET /api/v1/system/menus/tree` 返回完整的菜单树，前端根据 `menu_type` 递归渲染：

```json
{
    "code": 200,
    "data": [
        {
            "id": 0,
            "name": "首页",
            "menu_type": "C",
            "path": "/dashboard",
            "component": "dashboard/index",
            "icon": "HomeFilled",
            "is_frame": false,
            "is_cache": false,
            "visible": true,
            "children": []
        },
        {
            "id": 1,
            "name": "系统管理",
            "menu_type": "M",
            "path": "/system",
            "icon": "Setting",
            "visible": true,
            "children": [
                {
                    "id": 4,
                    "name": "用户管理",
                    "menu_type": "C",
                    "path": "/system/user",
                    "component": "system/user/index",
                    "perms": "system:user:list",
                    "visible": true,
                    "children": [
                        {
                            "id": 18,
                            "name": "用户查询",
                            "menu_type": "F",
                            "perms": "system:user:list",
                            "visible": true,
                            "children": []
                        }
                    ]
                }
            ]
        }
    ]
}
```

### 8.3 前端渲染逻辑

```
for each node in menu_tree:
    if node.menu_type == "M":
        render <el-sub-menu> (可折叠分组)
        recursively render children
    else if node.menu_type == "C":
        render <el-menu-item> (路由链接)
    else if node.menu_type == "F":
        render <el-button v-if="hasPerm(node.perms)"> (权限按钮)
```

### 8.4 菜单缓存

菜单树支持 Redis 缓存（key: `sys_menu:tree`，TTL: 1 小时），创建/更新/删除菜单时自动失效。

---

## 9. 数据权限体系

RyFrame 支持基于角色的数据权限控制（`data_scope` 字段）：

### 9.1 数据范围

| data_scope | 含义 | SQL 过滤条件 |
|------------|------|-------------|
| `1` | 全部数据 | 无限制 |
| `2` | 自定义部门 | `WHERE dept_id IN (select dept_id from sys_role_dept where role_id = ?)` |
| `3` | 本部门 | `WHERE dept_id = ?` |
| `4` | 本部门及以下 | `WHERE dept_id IN (本部门 + 子部门)` |
| `5` | 仅本人 | `WHERE id = ?` |

### 9.2 权限校验流程

```
请求 → AuthMiddleware（解码 JWT，提取 Claims.perms）
     → PermissionMiddleware（验证 Claims.perms 包含所需权限码）
     → Handler
       → Service
         → DataScope 过滤（根据 Claims.roles 查询 data_scope 并构建 SQL 条件）
```

---

## 10. 安全特性

### 10.1 多层限流

| 层级 | 维度 | 配置位置 | 配置节 |
|------|------|----------|--------|
| 第一层 | IP 级全局 | `app.rs` 全局 | `[rate_limit]` |
| 第二层 | 接口级 | `app.rs` 全局 | `[rate_limit.api_limits]` |
| 第三层 | 用户级 | system_router / tools_router | `[rate_limit]` user 相关 |

**配置示例** (`app.toml`)：

```toml
[rate_limit]
capacity = 100
refill_per_sec = 10
window_secs = 60
enable_user_rate_limit = true
user_capacity = 500
user_window_secs = 60
api_limits = { "POST /api/v1/auth/login" = 5 }
```

### 10.2 安全响应头

| 响应头 | 值 |
|--------|-----|
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `SAMEORIGIN` |
| `X-XSS-Protection` | `1; mode=block` |
| `Strict-Transport-Security` | `max-age=31536000` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |

### 10.3 其他安全特性

- **请求体大小限制**: 全局 10MB
- **请求超时**: 全局 30 秒
- **JWT 主动撤销**: 登出/强制下线时 token 加入黑名单
- **密码哈希**: argon2id（内存硬哈希，抗 GPU/ASIC 攻击）
- **日志脱敏**: password、token、secret 等敏感字段自动脱敏

---

## 11. 操作日志

POST/PUT/DELETE 请求通过 `oper_log_middleware` 自动记录到 `sys_oper_log` 表。

**记录内容**：

| 字段 | 说明 |
|------|------|
| `title` | 模块标题（如"用户管理"） |
| `business_type` | 业务类型（INSERT/UPDATE/DELETE/EXPORT/IMPORT/GRANT） |
| `method` | 操作方法（类名.方法名） |
| `request_method` | 请求方式（GET/POST/PUT/DELETE） |
| `oper_name` | 操作人（从 JWT Claims.username 提取） |
| `oper_url` | 请求 URL |
| `oper_ip` | 操作 IP |
| `oper_param` | 请求参数 JSON（截断至 2000 字符） |
| `json_result` | 响应结果 JSON |
| `status` | 操作状态（0=失败 1=成功） |
| `error_msg` | 错误信息 |
| `cost_time` | 耗时（毫秒） |

**说明**：文件上传路由（`/api/v1/common/upload/*`）已经接入认证、用户上下文和操作日志；大文件请求体处理由上传 handler 控制。

---

## 12. 缓存使用

```rust,ignore
use ryframe_core::cache::{Cache, BreakdownGuard};

// 基础读写
cache.set("user:1", &user, 3600).await?;
let user: Option<User> = cache.get("user:1").await?;

// Get-or-Load（自动回源，防缓存击穿）
let user = cache.get_or_load("user:1", 3600, || db.find_user(1)).await?;

// 防击穿双检锁
let guard = BreakdownGuard::new(redis_cache);
let user = guard.get_or_load_guarded("hot:key", 3600, || db.query()).await?;
```

**已使用缓存的模块**：

| 模块 | Key | TTL | 说明 |
|------|-----|-----|------|
| 菜单树 | `sys_menu:tree` | 3600s | 菜单 CRUD 时自动失效 |
| 部门树 | `sys_dept:tree` | 3600s | 部门 CRUD 时自动失效 |

---

## 13. 消息队列

```rust,ignore
use ryframe_core::message_queue::{MqBackend, create_in_memory_mq, publish_json};

let mq = create_in_memory_mq();

// 订阅
mq.subscribe("user.created", |msg| async move {
    // 处理用户创建事件（如发送欢迎邮件）
    Ok(())
}).await?;

// 发布
publish_json(&mq, "user.created", &user_data).await?;
```

---

## 14. 功能开关

```rust,ignore
use ryframe_core::feature_flag::FeatureFlags;

let flags = FeatureFlags::new()
    .with_flag("new_payment", false, "新支付模块");

if flags.is_enabled("new_payment") {
    // 新功能逻辑
}
```

---

## 15. gRPC 通信

框架集成 tonic，支持 gRPC 微服务间通信：

```rust,ignore
use ryframe_core::grpc::{GrpcServer, GrpcServerConfig, GrpcClient, GrpcClientConfig};

// 服务端
let config = GrpcServerConfig::default();
let server = GrpcServer::new(config);
let shutdown = server.serve(my_service).await?;

// 客户端
let client_config = GrpcClientConfig::new("http://localhost:50051");
let channel = GrpcClient::connect(&client_config).await?;
```

---

## 16. 多租户

通过请求头 `X-Tenant-Id` 识别租户：

```rust,ignore
use ryframe_core::multi_tenant::{TenantConfig, ExtractionMethod, tenant_middleware};

let config = TenantConfig {
    extraction_method: ExtractionMethod::Header("X-Tenant-Id".into()),
    isolation_strategy: IsolationStrategy::SharedTable,  // 共享表 + tenant_id 列
    default_tenant: None,
};
```

---

## 17. API 版本管理

```rust,ignore
use ryframe_core::versioning::{ApiVersion, VersionedRouter};

let v1_routes = Router::new().route("/users", get(v1_handler));
let v2_routes = Router::new().route("/users", get(v2_handler));

let router = VersionedRouter::new()
    .with_v1(v1_routes)
    .with_v2(v2_routes)
    .into_router();
```

- 版本号通过 URL 路径指定：`/api/v1/...` 和 `/api/v2/...`
- 请求头 `X-API-Version: v2` 也可指定版本
