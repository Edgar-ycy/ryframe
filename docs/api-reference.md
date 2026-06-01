# RyFrame API 参考文档

> **Base URL**: `http://localhost:8080`
> **API Prefix**: `/api/v1`
> **OpenAPI 3.0 文档**: [GET /api/v1/api-docs/openapi.json]
> **Swagger UI**: [GET /api/v1/swagger-ui]
> **生成日期**: 2026-06-01

---

## 目录

- [通用说明](#通用说明)
- [1. 认证 (Auth)](#1-认证-auth)
- [2. 个人中心 (Profile)](#2-个人中心-profile)
- [3. 用户管理 (User)](#3-用户管理-user)
- [4. 角色管理 (Role)](#4-角色管理-role)
- [5. 权限管理 (Permission)](#5-权限管理-permission)
- [6. 部门管理 (Dept)](#6-部门管理-dept)
- [7. 岗位管理 (Post)](#7-岗位管理-post)
- [8. 菜单管理 (Menu)](#8-菜单管理-menu)
- [9. 字典管理 (Dict)](#9-字典管理-dict)
- [10. 参数配置 (Config)](#10-参数配置-config)
- [11. 通知公告 (Notice)](#11-通知公告-notice)
- [12. 定时任务 (Job)](#12-定时任务-job)
- [13. 操作日志 (OperLog)](#13-操作日志-operlog)
- [14. 登录日志 (LoginLog)](#14-登录日志-loginlog)
- [15. 在线用户 (Online)](#15-在线用户-online)
- [16. 服务器监控 (Monitor)](#16-服务器监控-monitor)
- [17. 代码生成 (Generator)](#17-代码生成-generator)
- [18. 通用功能 (Common)](#18-通用功能-common)
- [19. 系统端点](#19-系统端点)

---

## 通用说明

### 统一响应格式

所有 JSON 接口均返回以下格式：

```jsonc
// 成功 - 有数据
{
    "code": 200,
    "msg": "操作成功",
    "data": { /* ... */ }
}

// 成功 - 无数据
{
    "code": 200,
    "msg": "操作成功"
}

// 失败
{
    "code": 500,
    "msg": "错误描述"
}
```

### 分页响应格式

分页接口返回格式：

```json
{
    "code": 200,
    "msg": "查询成功",
    "rows": [
        /* ... */
    ],
    "total": 100
}
```

### 分页请求参数

所有分页接口支持以下 Query 参数：

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码，默认 1 |
| `pageSize` | u64 | 否 | 每页条数，默认 10 |

### 认证方式

- Bearer Token：在请求头 `Authorization: Bearer <access_token>` 中传递
- Token 获取：[POST /api/v1/auth/login](#登录)
- Token 刷新：[POST /api/v1/auth/refresh](#刷新令牌)
- Token 过期：`access_token` 过期 1h，`refresh_token` 过期 168h（7 天）

### 常见状态码

| 状态码 | 含义 |
|--------|------|
| 200 | 操作成功 |
| 400 | 请求参数错误 |
| 401 | 未认证（令牌无效或已过期） |
| 403 | 无权限 |
| 404 | 资源不存在 |
| 409 | 数据冲突 |
| 429 | 请求过于频繁（限流） |
| 500 | 服务器内部错误 |

---

## 1. 认证 (Auth)

> Route Prefix: `/api/v1/auth`

### 登录

获取 access_token 和 refresh_token。登录失败有暴力破解防护（Redis 失败计数 + 锁定）。

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/auth/login` |
| **认证** | 否 |
| **限流** | 5 req/min（接口级） |

**Request Body** (`application/json`):

```json
{
    "username": "admin",
    "password": "123456"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `username` | string | 是 | 用户名 |
| `password` | string | 是 | 密码 |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "access_token": "eyJhbGciOiJIUzI1NiJ9...",
        "refresh_token": "eyJhbGciOiJIUzI1NiJ9...",
        "user_info": {
            "id": 1,
            "username": "admin",
            "nickname": "管理员",
            "email": "admin@example.com",
            "phone": "13800138000",
            "avatar": null,
            "roles": ["admin"],
            "perms": ["*:*:*"]
        }
    }
}
```

> **注意**: `data` 中不包含 `token_type` 和 `expires_in` 字段。前端可根据 JWT `access_token` 自行解码获取过期时间。

**错误响应** (401):

```json
{
    "code": 401,
    "msg": "用户名或密码错误"
}
```

---

### 刷新令牌

使用 refresh_token 获取新的 access_token。

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/auth/refresh` |
| **认证** | 否 |

**Request Body**:

```json
{
    "refresh_token": "eyJhbGciOiJIUzI1NiJ9..."
}
```

**成功响应** (200): 同登录成功响应

**错误响应** (401):

```json
{
    "code": 401,
    "msg": "令牌无效或已过期"
}
```

---

### 登出

将当前 token 加入黑名单，实现主动撤销。

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/auth/logout` |
| **认证** | 是 |

**Request Header**:

```
Authorization: Bearer <access_token>
```

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功"
}
```

---

### 获取当前用户信息

返回当前登录用户的基本信息、角色列表和权限列表。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/auth/me` |
| **认证** | 是 |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "id": 1,
        "username": "admin",
        "nickname": "管理员",
        "email": "admin@example.com",
        "phone": "13800138000",
        "avatar": null,
        "roles": ["admin"],
        "perms": ["*:*:*"]
    }
}
```

> **注意**: `/me` 返回字段 `perms`（非 `permissions`），且不包含 `menus` 菜单树。

---

### 获取验证码图片

返回验证码图片（PNG Base64）。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/auth/captcha/generate` |
| **认证** | 否 |
| **限流** | 10 req/min（IP 级） |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `captcha_type` | string | 否 | 验证码类型：`alphanumeric`（字母数字）/ `math`（数学计算），默认 `alphanumeric` |

**成功响应** (200):

```json
{
    "code": 200,
    "data": {
        "captcha_id": "0192a8b0-7c3d-7a1f-b4e5-abcdef123456",
        "image_base64": "data:image/png;base64,iVBORw0KGgo..."
    }
}
```

---

### 校验验证码

校验验证码是否正确（一次性使用，校验后立即删除）。

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/auth/captcha/verify` |
| **认证** | 否 |
| **限流** | 5 req/min（IP 级） |

**Request Body**:

```json
{
    "captcha_id": "0192a8b0-7c3d-7a1f-b4e5-abcdef123456",
    "code": "aB3d"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `captcha_id` | string | 是 | 验证码 ID（来自 generate 接口） |
| `code` | string | 是 | 用户输入的验证码（不区分大小写） |

**成功响应** (200):

```json
{
    "code": 200,
    "data": { "valid": true }
}
```

**错误响应** (400):

```json
{
    "code": 400,
    "msg": "验证码错误或已过期"
}
```

---

## 2. 个人中心 (Profile)

> Route Prefix: `/api/v1/auth/profile`
> **全部需要认证**

### 获取个人信息

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/auth/profile` |
| **认证** | 是 |

**成功响应** (200):

```json
{
    "code": 200,
    "data": {
        "id": 1,
        "username": "admin",
        "nickname": "管理员",
        "email": "admin@example.com",
        "phone": "13800138000",
        "avatar": null,
        "dept_name": "总公司",
        "roles": ["管理员"],
        "posts": ["CEO"]
    }
}
```

---

### 更新个人信息

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/auth/profile` |
| **认证** | 是 |

**Request Body**:

```json
{
    "nickname": "新昵称",
    "email": "admin@example.com",
    "phone": "13800138000",
    "sex": "0"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `nickname` | string | 是 | 昵称 |
| `email` | string | 否 | 邮箱 |
| `phone` | string | 否 | 手机号 |
| `sex` | string | 否 | 性别：`0`=未知, `1`=男, `2`=女 |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "个人信息更新成功"
}
```

---

### 修改密码

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/auth/profile/password` |
| **认证** | 是 |

**Request Body**:

```json
{
    "old_password": "123456",
    "new_password": "NewP@ssw0rd"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `old_password` | string | 是 | 旧密码 |
| `new_password` | string | 是 | 新密码（需满足复杂度要求） |

> 密码复杂度：长度 6-100 位，至少包含字母和数字（通过 `ChangePasswordRequest::validate_passwords` 校验）

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "密码修改成功"
}
```

---

### 更新头像

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/auth/profile/avatar` |
| **认证** | 是 |

**Request Body**:

```json
{
    "avatar_url": "/uploads/avatar/xxx.png"
}
```

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "头像更新成功"
}
```

---

## 3. 用户管理 (User)

> Route Prefix: `/api/v1/system/users`
> **全部需要认证**

### 用户列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/users` 或 `/api/v1/system/users/list` |
| **认证** | 是 |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码，默认 1 |
| `pageSize` | u64 | 否 | 每页条数，默认 10 |
| `username` | string | 否 | 用户名（模糊搜索） |
| `phone` | string | 否 | 手机号（模糊搜索） |
| `status` | string | 否 | 状态：`0`=正常, `1`=停用 |
| `dept_id` | i64 | 否 | 部门ID |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "查询成功",
    "rows": [
        {
            "id": 1,
            "username": "admin",
            "nickname": "管理员",
            "email": "admin@example.com",
            "phone": "13800138000",
            "status": "0",
            "dept_id": 1,
            "dept_name": "总公司",
            "remark": null,
            "created_at": "2026-01-01T00:00:00Z"
        }
    ],
    "total": 1
}
```

---

### 用户列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/users/listNoPage` |
| **认证** | 是 |

**响应**: 返回全部用户数组（最多 10000 条）。

---

### 用户详情

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/users/{id}` |
| **认证** | 是 |

**Path 参数**:

| 参数 | 类型 | 说明 |
|------|------|------|
| `id` | i64 | 用户 ID |

**成功响应** (200):

```json
{
    "code": 200,
    "data": {
        "id": 1,
        "username": "admin",
        "nickname": "管理员",
        "email": "admin@example.com",
        "phone": "13800138000",
        "status": "0",
        "dept_id": 1,
        "dept_name": "总公司",
        "role_names": ["管理员"],
        "role_ids": [1],
        "post_names": ["CEO"]
    }
}
```

---

### 创建用户

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/users` |
| **认证** | 是 |

**Request Body**:

```json
{
    "username": "zhangsan",
    "password": "P@ssw0rd",
    "nickname": "张三",
    "email": "zhangsan@example.com",
    "phone": "13900001111",
    "dept_id": 1,
    "role_ids": [2]
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `username` | string | 是 | 用户名（唯一） |
| `password` | string | 是 | 密码 |
| `nickname` | string | 是 | 昵称 |
| `email` | string | 否 | 邮箱 |
| `phone` | string | 否 | 手机号 |
| `dept_id` | i64 | 否 | 部门 ID |
| `role_ids` | i64[] | 否 | 角色 ID 列表 |

**成功响应** (200): 返回创建的用户 VO。

---

### 更新用户

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/users/{id}` |
| **认证** | 是 |

**Request Body**:

```json
{
    "nickname": "张三改",
    "email": "zhangsan_new@example.com",
    "phone": "13900002222",
    "dept_id": 2,
    "status": "0",
    "role_ids": [2, 3]
}
```

---

### 删除用户

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/users/{id}` |
| **认证** | 是 |

---

### 批量删除用户

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/users/batch/{ids}` |
| **认证** | 是 |

**Path 参数**: `ids` 为逗号分隔的 ID 列表，如 `/batch/1,2,3`

---

### 重置密码

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/users/{id}/password` |
| **认证** | 是 |

**Request Body**:

```json
{
    "password": "NewP@ssw0rd123"
}
```

---

### 修改用户状态

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/users/changeStatus` |
| **认证** | 是 |

**Request Body**:

```json
{
    "user_id": 1,
    "status": "1"
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `user_id` | i64 | 用户 ID |
| `status` | string | `0`=正常, `1`=停用 |

---

### 导出用户 (Excel)

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/users/export` |
| **认证** | 是 |
| **响应** | `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`（二进制 Excel 文件） |

---

### 导入用户 (Excel)

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/users/import` |
| **认证** | 是 |
| **Content-Type** | `multipart/form-data` |

**表单字段**:

| 字段 | 类型 | 说明 |
|------|------|------|
| `file` | file | 用户数据 Excel 文件 |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "导入完成",
    "data": {
        "success_count": 5,
        "fail_count": 1,
        "errors": ["第 3 行数据验证失败: ..."]
    }
}
```

---

### 下载导入模板

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/users/import-template` |
| **认证** | 是 |
| **响应** | Excel 模板文件 |

---

## 4. 角色管理 (Role)

> Route Prefix: `/api/v1/system/roles`
> **全部需要认证**

### 角色列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/roles` 或 `/api/v1/system/roles/list` |
| **认证** | 是 |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |
| `name` | string | 否 | 角色名称（模糊搜索） |
| `code` | string | 否 | 角色编码（模糊搜索） |
| `status` | string | 否 | 状态 |

---

### 角色列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/roles/listNoPage` |

---

### 角色详情

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/roles/{id}` |

---

### 创建角色

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/roles` |

**Request Body**:

```json
{
    "name": "普通用户",
    "code": "user",
    "sort": 1,
    "data_scope": "2"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | 角色名称 |
| `code` | string | 是 | 角色编码（唯一） |
| `sort` | i32 | 否 | 排序 |
| `data_scope` | string | 否 | 数据权限范围：`1`=全部, `2`=自定义, `3`=本部门, `4`=本部门及以下, `5`=仅本人 |

---

### 更新角色

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/roles/{id}` |

**Request Body**:

```json
{
    "name": "普通用户",
    "sort": 2,
    "status": "0",
    "data_scope": "2"
}
```

---

### 删除角色

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/roles/{id}` |

---

### 批量删除角色

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/roles/batch/{ids}` |

---

### 分配权限

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/roles/{id}/permissions` |

**Request Body**:

```json
{
    "perm_ids": [1, 2, 3, 4]
}
```

---

### 分配菜单

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/roles/{id}/menus` |

**Request Body**:

```json
{
    "menu_ids": [1, 2, 3]
}
```

---

### 设置数据权限

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/roles/{id}/data-scope` |

**Request Body**:

```json
{
    "data_scope": "2",
    "dept_ids": [1, 2, 3]
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `data_scope` | string | 是 | 数据权限范围（`1`-`5`） |
| `dept_ids` | i64[] | 否 | 自定义时关联的部门 ID 列表 |

---

### 导出角色 (Excel)

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/roles/export` |
| **响应** | Excel 文件 |

---

## 5. 权限管理 (Permission)

> Route Prefix: `/api/v1/system/permissions`
> **全部需要认证**

### 权限树

返回完整权限树（用于角色权限分配）。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/permissions/tree` |
| **认证** | 是 |

**成功响应** (200):

```json
{
    "code": 200,
    "data": [
        {
            "id": 1,
            "name": "系统管理",
            "code": "system",
            "children": [
                {
                    "id": 2,
                    "name": "用户管理",
                    "code": "system:user",
                    "children": [
                        { "id": 3, "name": "用户查询", "code": "system:user:list" },
                        { "id": 4, "name": "用户新增", "code": "system:user:create" }
                    ]
                }
            ]
        }
    ]
}
```

---

## 6. 部门管理 (Dept)

> Route Prefix: `/api/v1/system/depts`
> **全部需要认证**

### 部门树

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/depts/tree` |
| **认证** | 是 |

**成功响应** (200):

```json
{
    "code": 200,
    "data": [
        {
            "id": 1,
            "name": "总公司",
            "parent_id": 0,
            "sort": 1,
            "children": [
                {
                    "id": 2,
                    "name": "技术部",
                    "parent_id": 1,
                    "sort": 1,
                    "children": []
                }
            ]
        }
    ]
}
```

---

### 部门列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/depts/list` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |
| `name` | string | 否 | 名称（模糊搜索） |
| `status` | string | 否 | 状态 |

---

### 部门列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/depts/listNoPage` |

---

### 部门详情

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/depts/{id}` |

---

### 创建部门

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/depts` |

**Request Body**:

```json
{
    "name": "技术部",
    "parent_id": 1,
    "sort": 1
}
```

---

### 更新部门

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/depts/{id}` |

**Request Body**:

```json
{
    "name": "研发部",
    "parent_id": 1,
    "sort": 2,
    "status": "0"
}
```

---

### 删除部门

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/depts/{id}` |

---

## 7. 岗位管理 (Post)

> Route Prefix: `/api/v1/system/posts`
> **全部需要认证**

### 岗位列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/posts` 或 `/api/v1/system/posts/list` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |
| `name` | string | 否 | 名称（模糊搜索） |
| `code` | string | 否 | 编码（模糊搜索） |
| `status` | string | 否 | 状态 |

---

### 岗位列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/posts/listNoPage` |

---

### 岗位详情

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/posts/{id}` |

---

### 创建岗位

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/posts` |

**Request Body**:

```json
{
    "name": "项目经理",
    "code": "pm",
    "sort": 1
}
```

---

### 更新岗位

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/posts/{id}` |

**Request Body**:

```json
{
    "name": "项目经理",
    "sort": 2,
    "status": "0"
}
```

---

### 删除岗位

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/posts/{id}` |

---

### 导出岗位 (Excel)

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/posts/export` |

---

## 8. 菜单管理 (Menu)

> Route Prefix: `/api/v1/system/menus`
> **全部需要认证**

### 菜单树

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/menus/tree` |
| **认证** | 是 |

**成功响应** (200):

```json
{
    "code": 200,
    "data": [
        {
            "id": 1,
            "name": "系统管理",
            "parent_id": 0,
            "path": "/system",
            "component": "Layout",
            "icon": "system",
            "sort": 1,
            "visible": true,
            "children": [
                {
                    "id": 2,
                    "name": "用户管理",
                    "parent_id": 1,
                    "path": "user",
                    "component": "system/user/index",
                    "sort": 1,
                    "visible": true,
                    "children": []
                }
            ]
        }
    ]
}
```

---

### 菜单列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/menus/list` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |
| `name` | string | 否 | 名称（模糊搜索） |
| `status` | string | 否 | 状态 |

---

### 菜单列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/menus/listNoPage` |

---

### 菜单详情

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/menus/{id}` |

---

### 创建菜单

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/menus` |

**Request Body**:

```json
{
    "name": "用户管理",
    "parent_id": 1,
    "path": "user",
    "component": "system/user/index",
    "icon": "user",
    "sort": 1,
    "visible": true
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | 菜单名称 |
| `parent_id` | i64 | 是 | 父菜单 ID（顶级为 0） |
| `path` | string | 否 | 路由路径 |
| `component` | string | 否 | 前端组件路径 |
| `icon` | string | 否 | 图标名称 |
| `sort` | i32 | 否 | 排序 |
| `visible` | bool | 否 | 是否可见，默认 true |

---

### 更新菜单

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/menus/{id}` |

**Request Body**: 同创建，额外支持 `status` 字段。

---

### 删除菜单

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/menus/{id}` |

---

## 9. 字典管理 (Dict)

> Route Prefix: `/api/v1/system/dict`
> **全部需要认证**

### 字典类型列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/dict/types` 或 `/api/v1/system/dict/types/list` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |

---

### 字典类型列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/dict/types/listNoPage` |

---

### 创建字典类型

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/dict/types` |

**Request Body**:

```json
{
    "name": "用户状态",
    "code": "sys_user_status"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | 字典名称 |
| `code` | string | 是 | 字典编码（唯一） |

---

### 更新字典类型

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/dict/types/{id}` |

**Request Body**:

```json
{
    "name": "用户状态",
    "status": "0"
}
```

---

### 删除字典类型

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/dict/types/{id}` |

---

### 导出字典类型 (Excel)

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/dict/types/export` |

---

### 查询字典数据（按类型编码 Query）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/dict/data?type_code=sys_user_status` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `type_code` | string | 是 | 字典类型编码 |

**成功响应** (200):

```json
{
    "code": 200,
    "data": [
        { "id": 1, "label": "正常", "value": "0", "sort": 1, "status": "0" },
        { "id": 2, "label": "停用", "value": "1", "sort": 2, "status": "0" }
    ]
}
```

---

### 查询字典数据（按类型编码 Path）

前端适配端点，返回 `dictLabel` / `dictValue` 格式。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/dict/data/type/{dict_type}` |

**Path 参数**:

| 参数 | 类型 | 说明 |
|------|------|------|
| `dict_type` | string | 字典类型编码 |

**成功响应** (200):

```json
{
    "code": 200,
    "data": [
        { "dictLabel": "正常", "dictValue": "0", "cssClass": "" },
        { "dictLabel": "停用", "dictValue": "1", "cssClass": "danger" }
    ]
}
```

---

### 创建字典数据

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/dict/data` |

**Request Body**:

```json
{
    "type_code": "sys_user_status",
    "label": "正常",
    "value": "0",
    "sort": 1
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `type_code` | string | 是 | 字典类型编码 |
| `label` | string | 是 | 字典标签（显示值） |
| `value` | string | 是 | 字典键值 |
| `sort` | i32 | 否 | 排序 |

---

### 更新字典数据

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/dict/data/{id}` |

**Request Body**:

```json
{
    "label": "正常",
    "value": "0",
    "sort": 1,
    "status": "0"
}
```

---

### 删除字典数据

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/dict/data/{id}` |

---

## 10. 参数配置 (Config)

> Route Prefix: `/api/v1/system/configs`
> **全部需要认证**

### 参数列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/configs` 或 `/api/v1/system/configs/list` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |

---

### 参数列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/configs/listNoPage` |

---

### 按 Key 查询参数

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/configs/configKey/{key}` |

**成功响应** (200):

```json
{
    "code": 200,
    "data": "参数值"
}
```

---

### 参数详情

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/configs/{id}` |

---

### 创建参数

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/configs` |

**Request Body**:

```json
{
    "name": "用户默认密码",
    "key": "sys.user.initPassword",
    "value": "123456",
    "remark": "新建用户的默认密码"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | 参数名称 |
| `key` | string | 是 | 参数键名（唯一） |
| `value` | string | 是 | 参数键值 |
| `remark` | string | 否 | 备注 |

---

### 更新参数

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/configs/{id}` |

**Request Body**:

```json
{
    "value": "新值"
}
```

---

### 删除参数

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/configs/{id}` |

---

### 刷新缓存

清空所有参数配置的 Redis 缓存。

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/configs/refreshCache` |

---

### 导出参数 (Excel)

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/configs/export` |

---

## 11. 通知公告 (Notice)

> Route Prefix: `/api/v1/system/notices`
> **全部需要认证**

### 公告列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/notices` 或 `/api/v1/system/notices/list` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |
| `title` | string | 否 | 标题（模糊搜索） |
| `notice_type` | string | 否 | 公告类型（`1`=通知, `2`=公告） |
| `status` | string | 否 | 状态 |

---

### 公告列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/notices/listNoPage` |

---

### 公告详情

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/notices/{id}` |

---

### 创建公告

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/notices` |

**Request Body**:

```json
{
    "title": "系统维护通知",
    "content": "系统将于本周六凌晨进行维护...",
    "notice_type": "1"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `title` | string | 是 | 公告标题 |
| `content` | string | 是 | 公告内容 |
| `notice_type` | string | 否 | 公告类型：`1`=通知, `2`=公告 |

---

### 更新公告

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/notices/{id}` |

**Request Body**:

```json
{
    "title": "系统维护通知（更新）",
    "content": "...",
    "notice_type": "1",
    "status": "0"
}
```

---

### 删除公告

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/notices/{id}` |

---

## 12. 定时任务 (Job)

> Route Prefix: `/api/v1/system/jobs`
> **全部需要认证**

### 任务列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/jobs` 或 `/api/v1/system/jobs/listNoPage` |

---

### 任务列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/jobs/list` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |

---

### 创建任务

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/jobs` |

**Request Body**:

```json
{
    "name": "clean_oper_log",
    "cron_expr": "0 0 2 * * *",
    "group_name": "系统",
    "misfire_policy": "1",
    "concurrent": "0",
    "remark": "每天凌晨2点清理操作日志"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | 任务名称（对应注册的 handler） |
| `cron_expr` | string | 是 | Cron 表达式（6 位秒级） |
| `group_name` | string | 否 | 任务组名 |
| `misfire_policy` | string | 否 | 错过策略：`0`=忽略, `1`=立即执行一次 |
| `concurrent` | string | 否 | 是否并发：`0`=禁止, `1`=允许 |
| `remark` | string | 否 | 备注 |

---

### 更新任务

| 项目 | 内容 |
|------|------|
| **方法** | `PUT` |
| **路径** | `/api/v1/system/jobs/{id}` |

**Request Body**:

```json
{
    "cron_expr": "0 0 3 * * *",
    "status": "0",
    "remark": "修改为凌晨3点"
}
```

---

### 删除任务

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/jobs/{id}` |

---

### 暂停任务

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/jobs/{id}/pause` |

---

### 恢复任务

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/jobs/{id}/resume` |

---

### 立即触发一次

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/system/jobs/{id}/trigger` |

**成功响应** (200): 返回任务执行历史记录。

---

### 执行日志

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/jobs/logs` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |
| `job_name` | string | 否 | 任务名称 |
| `status` | string | 否 | 执行状态：`0`=成功, `1`=失败 |

---

## 13. 操作日志 (OperLog)

> Route Prefix: `/api/v1/system/operlogs`
> **全部需要认证**

### 日志列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/operlogs` 或 `/api/v1/system/operlogs/list` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |
| `oper_name` | string | 否 | 操作人（模糊搜索） |
| `status` | string | 否 | 操作状态：`0`=成功, `1`=失败 |
| `begin_time` | string | 否 | 开始时间（ISO 8601） |
| `end_time` | string | 否 | 结束时间（ISO 8601） |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "查询成功",
    "rows": [
        {
            "id": 1,
            "title": "用户管理",
            "business_type": "UPDATE",
            "oper_name": "admin",
            "oper_url": "/api/v1/system/users/1",
            "oper_ip": "192.168.1.1",
            "status": "0",
            "cost_time": 45,
            "oper_time": "2026-01-01T12:00:00Z"
        }
    ],
    "total": 100
}
```

---

### 日志列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/operlogs/listNoPage` |

---

### 清空日志

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/operlogs/clean` |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "成功清空 150 条操作日志"
}
```

---

### 导出日志 (Excel)

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/operlogs/export` |
| **Query** | 同 list 查询参数 |
| **响应** | Excel 文件 |

---

## 14. 登录日志 (LoginLog)

> Route Prefix: `/api/v1/system/loginlogs`
> **全部需要认证**

### 日志列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/loginlogs` 或 `/api/v1/system/loginlogs/list` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `page` | u64 | 否 | 页码 |
| `pageSize` | u64 | 否 | 每页条数 |
| `user_name` | string | 否 | 用户名（模糊搜索） |
| `status` | string | 否 | 状态：`0`=成功, `1`=失败 |
| `begin_time` | string | 否 | 开始时间 |
| `end_time` | string | 否 | 结束时间 |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "查询成功",
    "rows": [
        {
            "id": 1,
            "user_name": "admin",
            "ipaddr": "192.168.1.1",
            "login_location": null,
            "browser": "Chrome",
            "os": "Windows 10",
            "status": "0",
            "msg": null,
            "login_time": "2026-01-01T12:00:00Z"
        }
    ],
    "total": 50
}
```

---

### 日志列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/loginlogs/listNoPage` |

---

### 清空日志

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/loginlogs/clean` |

---

### 导出日志 (Excel)

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/loginlogs/export` |
| **Query** | 同 list 查询参数 |

---

## 15. 在线用户 (Online)

> Route Prefix: `/api/v1/system/online`
> **全部需要认证**

### 在线用户列表（不分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/online` 或 `/api/v1/system/online/listNoPage` |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `username` | string | 否 | 用户名（模糊搜索） |
| `ipaddr` | string | 否 | IP 地址（模糊搜索） |

**成功响应** (200):

```json
{
    "code": 200,
    "data": [
        {
            "token_id": "abc123...",
            "user_id": 1,
            "username": "admin",
            "dept_name": "总公司",
            "ipaddr": "192.168.1.1",
            "login_location": null,
            "browser": "Chrome",
            "os": "Windows 10",
            "login_time": "2026-01-01T12:00:00Z",
            "last_access_time": "2026-01-01T12:30:00Z"
        }
    ]
}
```

---

### 在线用户列表（分页）

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/system/online/list` |

**Query 参数**: 同上，额外支持 `page` 和 `pageSize`。

---

### 强制下线

| 项目 | 内容 |
|------|------|
| **方法** | `DELETE` |
| **路径** | `/api/v1/system/online/{token_id}` |

**Path 参数**:

| 参数 | 类型 | 说明 |
|------|------|------|
| `token_id` | string | Token JTI（来自在线用户列表的 `token_id`） |

---

## 16. 服务器监控 (Monitor)

> Route Prefix: `/api/v1/monitor`
> 监控端点默认公开，生产环境应通过 Nginx 限制访问

### 服务器信息

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/server` |
| **认证** | 否 |

**成功响应** (200):

```json
{
    "code": 200,
    "data": {
        "cpu": {
            "cpu_num": 8,
            "used": 25.5,
            "sys": 10.2,
            "free": 74.5
        },
        "memory": {
            "total": "16.0 GB",
            "used": "8.5 GB",
            "free": "7.5 GB",
            "usage": 53.1
        },
        "disk": {
            "total": "256.0 GB",
            "used": "120.0 GB",
            "free": "136.0 GB",
            "usage": 46.9
        }
    }
}
```

---

### 增强健康检查

检查数据库 + Redis 连通性。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/health` |
| **认证** | 否 |

**成功响应** (200):

```json
{
    "code": 200,
    "data": {
        "status": "UP",
        "database": "connected",
        "redis": "connected",
        "timestamp": "2026-01-01T12:00:00Z"
    }
}
```

---

### 缓存统计

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/cache` |
| **认证** | 否 |

---

### Redis 命令统计

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/cache/commands` |
| **认证** | 否 |

---

### 数据库连接池

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/db-pool` |
| **认证** | 否 |

**成功响应** (200):

```json
{
    "code": 200,
    "data": {
        "status": "connected",
        "active_connections": 5,
        "timestamp": "2026-01-01T12:00:00Z"
    }
}
```

---

### Prometheus Metrics

导出 Prometheus 格式的指标文本。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/metrics` |
| **认证** | 否（Nginx 限制内网） |
| **响应** | `text/plain; version=0.0.4` |

**主要指标**:

| 指标名 | 类型 | 说明 |
|--------|------|------|
| `ryframe_http_requests_total` | Counter | HTTP 请求总数（method/path/status） |
| `ryframe_http_request_duration_seconds` | Histogram | 请求延迟分布 |
| `ryframe_http_requests_in_flight` | Gauge | 当前并发请求数 |
| `ryframe_process_cpu_seconds_total` | Gauge | CPU 累计使用时间 |
| `ryframe_process_resident_memory_bytes` | Gauge | 常驻内存 |
| `ryframe_process_virtual_memory_bytes` | Gauge | 虚拟内存 |
| `ryframe_process_open_fds` | Gauge | 打开的文件描述符数 |
| `ryframe_process_threads` | Gauge | 线程数 |
| `ryframe_process_start_time_seconds` | Gauge | 进程启动时间戳 |

---

## 17. 代码生成 (Generator)

> Route Prefix: `/api/v1/tools/gen`
> **全部需要认证**

### 列出数据库表

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/tools/gen/tables` |
| **认证** | 是 |

---

### 预览生成内容

根据表名生成 entity、repository、dto、service、handler 五层代码（不写盘）。

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/tools/gen/preview` |
| **认证** | 是 |

**Request Body** (`application/json`):

```json
{
    "table_name": "sys_user",
    "module": "system",
    "author": "ryframe",
    "overwrite": false
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `table_name` | string | 是 | 数据库表名 |
| `module` | string | 是 | 模块名称（生成到 crates/ryframe-{module}/src/） |
| `author` | string | 否 | 作者名称 |
| `overwrite` | bool | 否 | 是否覆盖已存在文件，默认 `false` |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": [
        {
            "path": "crates/ryframe-system/src/entities/sys_user.rs",
            "content": "use sea_orm::entity::prelude::*;\n\n#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]\n// ..."
        },
        {
            "path": "crates/ryframe-system/src/repositories/sys_user_repo.rs",
            "content": "// repository code ..."
        },
        {
            "path": "crates/ryframe-system/src/dto/sys_user_dto.rs",
            "content": "// dto code ..."
        },
        {
            "path": "crates/ryframe-system/src/service/sys_user_service.rs",
            "content": "// service code ..."
        },
        {
            "path": "crates/ryframe-system/src/handlers/sys_user_handler.rs",
            "content": "// handler code ..."
        }
    ]
}
```

| 返回字段 | 类型 | 说明 |
|----------|------|------|
| `path` | string | 生成的相对文件路径 |
| `content` | string | 文件源代码内容 |

**错误响应** (400):

```json
{
    "code": 400,
    "msg": "表名包含非法字符"
}
```

---

### 生成代码（写盘）

生成代码并直接写入项目目录。已存在的文件默认跳过（除非设置 `overwrite: true`）。

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/tools/gen/generate` |
| **认证** | 是 |

**Request Body**: 同 [预览生成内容](#预览生成内容)

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": [
        "crates/ryframe-system/src/entities/sys_user.rs",
        "crates/ryframe-system/src/repositories/sys_user_repo.rs",
        "crates/ryframe-system/src/dto/sys_user_dto.rs",
        "crates/ryframe-system/src/service/sys_user_service.rs",
        "crates/ryframe-system/src/handlers/sys_user_handler.rs"
    ]
}
```

> `data` 返回成功写入的文件路径列表。

---

### 下载生成代码（ZIP）

生成代码并打包为 ZIP 文件下载。

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/tools/gen/download` |
| **认证** | 是 |

**Request Body** (`application/json`): 同 [预览生成内容](#预览生成内容)

**成功响应** (200):

- `Content-Type`: `application/zip`
- `Content-Disposition`: `attachment; filename="ryframe-gen.zip"`
- Body: ZIP 二进制数据

**错误响应** (400):

```json
{
    "code": 400,
    "msg": "表名包含非法字符"
}
```

---

## 18. 通用功能 (Common)

> Route Prefix: `/api/v1/common`

### 通用文件上传

上传任意类型文件。支持大小限制和多类型校验。

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/common/upload` |
| **认证** | 否 |
| **Content-Type** | `multipart/form-data` |

**Request Body** (`multipart/form-data`):

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file` | file | 是 | 上传的文件（表单字段名任意） |

**限制**:
- 最大文件大小：默认 10MB
- 允许的扩展名：默认通用文件类型

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "file_url": "/api/v1/common/download?path=uploads/20260101_abc123_file.pdf",
        "file_info": {
            "original_name": "report.pdf",
            "storage_name": "20260101_abc123_file.pdf",
            "file_path": "/uploads/20260101_abc123_file.pdf",
            "file_size": 2048576,
            "content_type": "application/pdf",
            "upload_time": "2026-01-01T12:00:00+08:00"
        }
    }
}
```

| 响应字段 | 类型 | 说明 |
|----------|------|------|
| `file_url` | string | 文件下载 URL |
| `file_info.original_name` | string | 原始文件名 |
| `file_info.storage_name` | string | 存储文件名（防冲突命名） |
| `file_info.file_path` | string | 服务器存储路径 |
| `file_info.file_size` | u64 | 文件大小（字节） |
| `file_info.content_type` | string | MIME 类型 |
| `file_info.upload_time` | string | 上传时间 (RFC3339) |

**错误响应** (400):

```json
{
    "code": 400,
    "msg": "文件大小超过限制（最大 10 MB）"
}
```

---

### 图片上传

专门上传图片文件，上传后自动压缩以节省存储空间和带宽。

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/common/upload/image` |
| **认证** | 否 |
| **Content-Type** | `multipart/form-data` |

**Request Body** (`multipart/form-data`):

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file` | file | 是 | 图片文件 |

**限制**:
- 最大文件大小：5MB
- 允许的格式：jpg / jpeg / png / gif / bmp / webp
- 上传后自动压缩

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "file_url": "/api/v1/common/download?path=uploads/20260101_def456_avatar.png",
        "file_info": {
            "original_name": "avatar.png",
            "storage_name": "20260101_def456_avatar.png",
            "file_path": "/uploads/20260101_def456_avatar.png",
            "file_size": 128000,
            "content_type": "image/png",
            "upload_time": "2026-01-01T12:00:00+08:00"
        }
    }
}
```

**错误响应** (400):

```json
{
    "code": 400,
    "msg": "图片大小超过限制（最大 5 MB）"
}
```

---

### 文件下载

根据路径下载已上传的文件。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/common/download` |
| **认证** | 是 |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `path` | string | 是 | 文件相对路径（来自上传响应中的 `file_path` 或 `file_url`） |

**成功响应** (200):

- 根据文件类型返回对应的 `Content-Type`
- `Content-Disposition`: `attachment; filename="..."`
- Body: 文件二进制数据

**错误响应** (400):

```json
{
    "code": 400,
    "msg": "非法的文件路径"
}
```

**错误响应** (404):

```json
{
    "code": 404,
    "msg": "文件不存在"
}
```

---

## 19. 系统端点

> 无需认证（公开访问）

### API 版本信息

返回 API 版本和可用端点列表。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/version` |
| **认证** | 否 |

**成功响应** (200):

```json
{
    "name": "ryframe-api",
    "version": "0.5.0",
    "api_prefix": "/api/v1",
    "endpoints": {
        "auth": "/api/v1/auth",
        "system": "/api/v1/system",
        "monitor": "/api/v1/monitor",
        "tools": "/api/v1/tools",
        "common": "/api/v1/common",
        "openapi": "/api/v1/api-docs/openapi.json",
        "swagger": "/api/v1/swagger-ui"
    }
}
```

---

### OpenAPI JSON 文档

返回标准的 OpenAPI 3.0 规范 JSON 文档，可供 Swagger UI、Redoc 等工具消费。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/api-docs/openapi.json` |
| **认证** | 否 |

**成功响应** (200):

- `Content-Type`: `application/json`
- Body: OpenAPI 3.0 标准 JSON

---

### Swagger UI 交互文档

提供基于 Swagger UI 的交互式 API 文档页面，可直接在浏览器中测试接口。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/swagger-ui` |
| **认证** | 否 |
| **Content-Type** | `text/html` |

**成功响应** (200): Swagger UI 页面 (HTML)

> Swagger UI 默认加载 `/api/v1/api-docs/openapi.json` 作为文档源。

---

## 附录 A：监控端点

> Route Prefix: `/api/v1/monitor`
> **认证要求**：监控端点通过 `ryframe-monitor` crate 注册，未在 router.rs 中统一添加认证中间件。生产环境建议通过 Nginx 限制 IP 访问。

### 健康检查

检查服务、数据库、Redis 的连通性状态。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/health` |
| **认证** | 否 |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "status": "UP",
        "database": "connected",
        "redis": "connected",
        "timestamp": "2026-01-01T12:00:00+08:00"
    }
}
```

| 响应字段 | 类型 | 说明 |
|----------|------|------|
| `status` | string | `UP` / `DOWN` |
| `database` | string | `connected` / `disconnected` |
| `redis` | string | `connected` / `disconnected` / `not_configured` |
| `timestamp` | string | 检查时间 (RFC3339) |

---

### 服务端资源信息

获取服务器运行时资源信息（CPU、内存、系统信息）。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/server` |
| **认证** | 否 |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "system_info": {
            "os": "Windows 11",
            "arch": "x86_64",
            "hostname": "dev-server"
        },
        "cpu": {
            "cores": 16,
            "usage_percent": 12.5
        },
        "memory": {
            "total_bytes": 34359738368,
            "used_bytes": 8589934592,
            "usage_percent": 25.0
        },
        "process": {
            "pid": 12345,
            "uptime_seconds": 86400,
            "memory_bytes": 134217728,
            "cpu_percent": 2.1
        }
    }
}
```

---

### 缓存信息

查看 Redis 缓存配置和状态。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/cache` |
| **认证** | 否 |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "redis_available": true,
        "info": {
            "connected_clients": 5,
            "used_memory_human": "2.5M",
            "uptime_in_seconds": 864000
        }
    }
}
```

> 若 Redis 未配置，`redis_available` 为 `false`。

---

### 缓存命令统计

查看 Redis 命令执行统计。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/cache/commands` |
| **认证** | 否 |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "get": 15230,
        "set": 845,
        "del": 120,
        "exists": 563
    }
}
```

---

### 数据库连接池状态

查看数据库连接池的活跃连接数和连通状态。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/db-pool` |
| **认证** | 否 |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "status": "connected",
        "active_connections": 8,
        "timestamp": "2026-01-01T12:00:00+08:00"
    }
}
```

---

### Prometheus Metrics

返回 Prometheus 格式的指标数据。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/monitor/metrics` |
| **认证** | 否 |
| **Content-Type** | `text/plain; version=0.0.4` |

**成功响应** (200):

```
# HELP ryframe_http_requests_total Total HTTP requests
# TYPE ryframe_http_requests_total counter
ryframe_http_requests_total{method="GET",path="/api/v1/auth/me",status="200"} 42

# HELP ryframe_http_request_duration_seconds HTTP request duration
# TYPE ryframe_http_request_duration_seconds histogram
ryframe_http_request_duration_seconds_bucket{le="0.005"} 100
ryframe_http_request_duration_seconds_bucket{le="0.01"} 250
ryframe_http_request_duration_seconds_bucket{le="+Inf"} 300

# HELP ryframe_process_resident_memory_bytes Resident memory in bytes
# TYPE ryframe_process_resident_memory_bytes gauge
ryframe_process_resident_memory_bytes 134217728
```

> 生产环境建议通过 Nginx 配置 `allow 10.0.0.0/8; deny all;` 限制仅 Prometheus 可访问此端点。

---

## 附录 B：验证码端点

> Route Prefix: `/api/v1/auth/captcha`
> **无需认证**

### 生成验证码

生成验证码图片并返回 Base64 编码。支持字母数字和数学计算两种类型。

| 项目 | 内容 |
|------|------|
| **方法** | `GET` |
| **路径** | `/api/v1/auth/captcha/generate` |
| **认证** | 否 |
| **限流** | 10 req/min（按 IP） |

**Query 参数**:

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `captcha_type` | string | 否 | 验证码类型：`alphanumeric`（默认）/ `math` |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "captcha_id": "0197c8d5-1234-7abc-def0-123456789abc",
        "image_base64": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAA..."
    }
}
```

| 响应字段 | 类型 | 说明 |
|----------|------|------|
| `captcha_id` | string | 验证码唯一标识（后续校验用） |
| `image_base64` | string | Base64 编码的 PNG 图片（可直接用于 `<img src>`） |

**错误响应** (429):

```json
{
    "code": 429,
    "msg": "验证码请求过于频繁，请稍后再试"
}
```

---

### 校验验证码

验证用户输入的验证码是否正确。验证码为一次性使用，校验后自动销毁。

| 项目 | 内容 |
|------|------|
| **方法** | `POST` |
| **路径** | `/api/v1/auth/captcha/verify` |
| **认证** | 否 |
| **限流** | 5 req/min（按 IP） |

**Request Body** (`application/json`):

```json
{
    "captcha_id": "0197c8d5-1234-7abc-def0-123456789abc",
    "code": "ABCD"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `captcha_id` | string | 是 | 生成验证码时返回的 ID |
| `code` | string | 是 | 用户输入的验证码（不区分大小写） |

**成功响应** (200):

```json
{
    "code": 200,
    "msg": "操作成功",
    "data": {
        "valid": true
    }
}
```

**错误响应** (400):

```json
{
    "code": 400,
    "msg": "验证码错误或已过期"
}
```

---

## 快速索引

### 按认证要求

| 认证要求 | 端点 |
|----------|------|
| **无需认证** | `/api/v1/auth/login`, `/api/v1/auth/refresh`, `/api/v1/auth/captcha/*`, `/api/v1/common/upload/*`, `/api/v1/version`, `/api/v1/api-docs/openapi.json`, `/api/v1/swagger-ui`, `/api/v1/monitor/*` |
| **需要认证** | `/api/v1/auth/logout`, `/api/v1/auth/me`, `/api/v1/auth/profile/*`, `/api/v1/system/*`, `/api/v1/tools/*`, `/api/v1/common/download` |

### 按限流策略

| 限流策略 | 端点 |
|----------|------|
| **严格（5 req/min）** | `/api/v1/auth/login`, `/api/v1/auth/captcha/verify` |
| **常规（10 req/min）** | `/api/v1/auth/captcha/generate` |
| **宽松（30 req/s）** | 全局 API 默认 |

### 按模块前缀

| 前缀 | 模块 |
|------|------|
| `/api/v1/auth/*` | 认证、验证码、个人中心 |
| `/api/v1/system/*` | 用户/角色/权限/部门/岗位/菜单/字典/参数/通知/任务/日志/在线用户 |
| `/api/v1/monitor/*` | 服务器/健康检查/缓存/数据库池/Metrics |
| `/api/v1/tools/gen/*` | 代码生成 |
| `/api/v1/common/*` | 文件上传/下载 |
| `/api/v1/version` | API 版本信息 |
| `/api/v1/api-docs/openapi.json` | OpenAPI 文档 |
| `/api/v1/swagger-ui` | Swagger UI 页面 |