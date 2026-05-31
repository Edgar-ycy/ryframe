# API 开发指南

## 项目结构

```
crates/
├── ryframe-api/      # API 层：路由、处理器、DTO、OpenAPI、操作日志中间件
│   ├── handlers/     # 19 个请求处理器
│   ├── dto/          # 15 个数据传输对象
│   ├── extractors/   # 自定义提取器
│   ├── router.rs     # 路由注册（auth/system/monitor/tools/common）
│   ├── openapi.rs    # utoipa OpenAPI 文档定义
│   ├── versioning.rs # API 版本协商
│   └── oper_log_middleware.rs  # 操作日志记录
├── ryframe-service/  # 服务层：业务逻辑
├── ryframe-db/       # 数据层：实体、仓库、迁移
│   ├── entities/     # 19 个数据库实体
│   └── repositories/ # 14 个数据仓库
├── ryframe-auth/     # 认证授权
├── ryframe-middleware/ # 15 个通用中间件
└── ryframe-core/     # 核心基础设施（缓存/消息队列/多租户/熔断器等）
```

## 路由约定

- 所有 API 以 `/api/v1` 为前缀
- 公开路由无需认证：`/api/v1/auth/login`、`/api/v1/auth/refresh`
- 受保护路由需要 JWT Token（`Authorization: Bearer <token>`）
- OpenAPI JSON：`/api/v1/api-docs/openapi.json`
- Swagger UI：`/api/v1/swagger-ui`

## 中间件执行顺序

**全局层**（`app.rs`，最外层先执行）：

```
RateLimit → ApiRateLimit → BodyLimit(10MB) → Timeout(30s) → XssFilter
  → RequestLog → CORS → Compression → RequestId → Telemetry → Metrics
```

**业务路由层**（system_router 示例）：

```
Auth → OnlineUserTracking → OperLog → Handler
```

## 认证流程

### 登录

```
POST /api/v1/auth/login
Content-Type: application/json

{
    "username": "admin",
    "password": "password123",
    "captcha_id": "uuid",
    "captcha_code": "1234"
}
```

响应：

```json
{
    "code": 200,
    "message": "success",
    "data": {
        "access_token": "eyJ...",
        "refresh_token": "eyJ...",
        "expires_in": 3600
    }
}
```

### 请求认证

```
Authorization: Bearer eyJhbGciOiJIUzI1NiIs...
```

### 刷新 Token

```
POST /api/v1/auth/refresh
Authorization: Bearer {refresh_token}
```

### 登出

```
POST /api/v1/auth/logout
Authorization: Bearer {token}
```

登出后 Token 被加入黑名单（基于 Redis 或内存），直到过期自动清理。

### 获取当前用户

```
GET /api/v1/auth/me
Authorization: Bearer {token}
```

## 完整 API 路由表

### 认证 (`/api/v1/auth`)

| 方法 | 路径 | 说明 | 认证 |
|------|------|------|------|
| `POST` | `/login` | 用户登录 | 否 |
| `POST` | `/refresh` | 刷新令牌 | 否 |
| `POST` | `/logout` | 用户登出 | 是 |
| `GET` | `/me` | 当前用户信息 + 菜单 + 角色 | 是 |

### 验证码 (`/api/v1/auth/captcha`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/image` | 获取图形验证码 |
| `POST` | `/verify` | 校验验证码 |

### 个人中心 (`/api/v1/auth/profile`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/` | 个人信息 |
| `PUT` | `/` | 更新个人信息 |
| `PUT` | `/password` | 修改密码 |
| `POST` | `/avatar` | 上传头像 |

### 系统管理 (`/api/v1/system`) — 全部需要认证

#### 用户管理 (`/users`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list` | 分页查询用户 |
| `GET` | `/:id` | 用户详情 |
| `POST` | `/` | 创建用户 |
| `PUT` | `/` | 更新用户 |
| `DELETE` | `/:id` | 删除用户 |
| `PUT` | `/reset-password` | 重置密码 |
| `PUT` | `/change-status` | 修改状态 |

#### 角色管理 (`/roles`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list` | 角色列表 |
| `GET` | `/:id` | 角色详情 |
| `POST` | `/` | 创建角色 |
| `PUT` | `/` | 更新角色 |
| `DELETE` | `/:id` | 删除角色 |
| `PUT` | `/assign-perms` | 分配权限 |
| `PUT` | `/assign-menus` | 分配菜单 |
| `PUT` | `/assign-data-scope` | 设置数据权限 |

#### 权限管理 (`/permissions`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/tree` | 权限树 |

#### 菜单管理 (`/menus`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/tree` | 菜单树 |
| `POST` | `/` | 创建菜单 |
| `PUT` | `/` | 更新菜单 |
| `DELETE` | `/:id` | 删除菜单 |

#### 部门管理 (`/depts`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/tree` | 部门树 |
| `POST` | `/` | 创建部门 |
| `PUT` | `/` | 更新部门 |
| `DELETE` | `/:id` | 删除部门 |

#### 岗位管理 (`/posts`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list` | 岗位列表 |
| `POST` | `/` | 创建岗位 |
| `PUT` | `/` | 更新岗位 |
| `DELETE` | `/:id` | 删除岗位 |

#### 参数配置 (`/configs`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list` | 配置列表 |
| `GET` | `/:id` | 配置详情 |
| `GET` | `/key/:key` | 按 Key 查询 |
| `POST` | `/` | 创建配置 |
| `PUT` | `/` | 更新配置 |
| `DELETE` | `/:id` | 删除配置 |

#### 字典管理 (`/dict`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/types` | 字典类型列表 |
| `GET` | `/types/:id` | 字典类型详情 |
| `POST` | `/types` | 创建字典类型 |
| `PUT` | `/types` | 更新字典类型 |
| `DELETE` | `/types/:id` | 删除字典类型 |
| `GET` | `/data/:type_code` | 按类型获取字典数据 |
| `POST` | `/data` | 创建字典数据 |
| `PUT` | `/data` | 更新字典数据 |
| `DELETE` | `/data/:id` | 删除字典数据 |

#### 通知公告 (`/notices`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list` | 通知列表 |
| `POST` | `/` | 创建通知 |
| `PUT` | `/` | 更新通知 |
| `DELETE` | `/:id` | 删除通知 |

#### 操作日志 (`/operlogs`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list` | 操作日志分页查询 |
| `DELETE` | `/clean` | 清空操作日志 |

#### 登录日志 (`/loginlogs`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list` | 登录日志分页查询 |
| `DELETE` | `/clean` | 清空登录日志 |

#### 定时任务 (`/jobs`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list` | 任务列表 |
| `POST` | `/` | 创建任务 |
| `PUT` | `/` | 更新任务 |
| `DELETE` | `/:id` | 删除任务 |
| `POST` | `/:id/pause` | 暂停任务 |
| `POST` | `/:id/resume` | 恢复任务 |
| `POST` | `/:id/trigger` | 立即执行一次 |

#### 在线用户 (`/online`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/list` | 在线用户列表 |
| `DELETE` | `/:id` | 强制踢出用户 |

### 监控 (`/api/v1/monitor`)

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/server` | 服务器 CPU/内存/磁盘 |
| `GET` | `/health` | 健康检查（DB/Redis） |

### 工具 (`/api/v1/tools`) — 全部需要认证

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/gen/tables` | 数据库表列表 |
| `POST` | `/gen/preview` | 预览生成代码 |
| `POST` | `/gen/generate` | 执行代码生成 |

### 通用 (`/api/v1/common`)

| 方法 | 路径 | 说明 | 认证 |
|------|------|------|------|
| `POST` | `/upload` | 文件上传 | 否 |
| `POST` | `/uploads` | 批量上传 | 否 |
| `GET` | `/download/*path` | 文件下载 | 是 |

### 文档

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/v1/api-docs/openapi.json` | OpenAPI JSON 文档 |
| `GET` | `/api/v1/swagger-ui` | Swagger UI 交互文档 |
| `GET` | `/api/v1/version` | API 版本信息 + 端点列表 |

## 统一响应格式

```json
{
    "code": 200,
    "message": "操作成功",
    "data": { ... }
}
```

**响应码规范**：

| 状态码 | 含义 |
|--------|------|
| 200 | 操作成功 |
| 400 | 请求参数错误 |
| 401 | 未认证（令牌无效/过期） |
| 403 | 无权限 / 缺少租户信息 |
| 404 | 资源不存在 |
| 409 | 数据冲突（唯一键重复 / 幂等性） |
| 429 | 请求过于频繁（限流） |
| 500 | 服务器内部错误 |

## 分页约定

**请求**：

```
GET /api/v1/system/users/list?page=1&page_size=10&sort_field=id&sort_order=desc
```

**响应**：

```json
{
    "code": 200,
    "message": "success",
    "data": {
        "items": [...],
        "total": 100,
        "page": 1,
        "page_size": 10,
        "total_pages": 10
    }
}
```

## API 版本管理

框架支持多版本 API 共存：

```rust
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

## 安全特性

### 多层限流

```rust
// 全局：令牌桶 / 固定窗口
let key = RateLimiter::ip_key(&addr);
limiter.acquire(&key, capacity, refill_per_sec).await;

// 用户级：500次/分钟/用户
let key = RateLimiter::user_key("user_123");
limiter.sliding_window_acquire(&key, 60, 500).await;

// 接口级：登录接口 5次/分钟
// 配置: [rate_limit.api_limits] "POST /api/v1/auth/login" = 5
```

### 请求体大小限制

全局 10MB，通过 `body_limit_middleware` 控制。

### 请求超时

全局 30 秒，通过 `timeout_middleware` 控制。

### 幂等性

防止重复提交，在请求头中携带唯一标识：

```
Idempotency-Key: unique-request-id-12345
```

### 重放防护

```
X-Timestamp: 1716768000
X-Nonce: random-nonce-value
```

### 安全响应头

| 响应头 | 值 |
|--------|-----|
| X-Content-Type-Options | nosniff |
| X-Frame-Options | SAMEORIGIN |
| X-XSS-Protection | 1; mode=block |
| Strict-Transport-Security | max-age=31536000 |
| Content-Security-Policy | 可配置 |
| Referrer-Policy | strict-origin-when-cross-origin |

### ETag 缓存

自动为 GET 响应生成 ETag（弱校验），支持 `If-None-Match` 返回 304。

### 日志脱敏

请求/响应日志自动脱敏敏感字段（password、token、secret 等）。

## 操作日志

POST/PUT/DELETE 请求自动记录到 `sys_oper_log` 表，包含：

- 操作人、IP、URL、请求方法
- 请求参数（截断至 2000 字符）
- 响应结果
- 耗时（毫秒）
- 业务类型

使用 DataDiff 记录字段变更：

```rust
use ryframe_common::utils::{DataDiff, DataDiffBuilder};

let diff = DataDiffBuilder::new()
    .change("name", old_name, new_name)
    .change("status", "0", "1")
    .build();
```

## 文件上传

```
POST /api/v1/common/upload
Content-Type: multipart/form-data

file: <binary>
```

支持本地存储和 MinIO/S3 对象存储，配置在 `[object_storage]` 节。

## 功能开关

运行时动态控制功能启用/禁用：

```rust
use ryframe_core::feature_flag::FeatureFlags;

let flags = FeatureFlags::new()
    .with_flag("new_feature", false, "新功能");

if flags.is_enabled("new_feature") {
    // 执行新功能逻辑
}
```

## gRPC 通信

框架集成 tonic，支持 gRPC 服务端/客户端：

```rust
use ryframe_core::grpc::{GrpcServer, GrpcServerConfig, GrpcClient, GrpcClientConfig};

// 服务端
let config = GrpcServerConfig::default();
let server = GrpcServer::new(config);
let shutdown = server.serve(my_service).await?;

// 客户端
let client_config = GrpcClientConfig::new("http://localhost:50051");
let channel = GrpcClient::connect(&client_config).await?;
```

## 多租户

通过请求头 `X-Tenant-Id` 识别租户：

```rust
use ryframe_core::multi_tenant::{TenantConfig, ExtractionMethod, tenant_middleware};

let config = TenantConfig {
    extraction_method: ExtractionMethod::Header("X-Tenant-Id".into()),
    isolation_strategy: IsolationStrategy::SharedTable,
    default_tenant: None,
};

Router::new()
    .layer(middleware::from_fn_with_state(Arc::new(config), tenant_middleware))
```

详见 `architecture.md` §6.9。

## 缓存使用

```rust
use ryframe_core::cache::{Cache, BreakdownGuard, RedisCache};

// 基础读写
cache.set("user:1", &user, 3600).await?;
let user: Option<User> = cache.get("user:1").await?;

// Get-or-Load（自动回源）
let user = cache.get_or_load("user:1", 3600, || db.find_user(1)).await?;

// 防击穿（双检锁）
let guard = BreakdownGuard::new(redis_cache);
let user = guard.get_or_load_guarded("hot:key", 3600, || db.query()).await?;
```

## 消息队列

```rust
use ryframe_core::message_queue::{MqBackend, create_in_memory_mq, publish_json};

let mq = create_in_memory_mq();

// 订阅
mq.subscribe("user.created", |msg| async move {
    // 处理消息
    Ok(())
}).await?;

// 发布 JSON
publish_json(&mq, "user.created", &user_data).await?;
```
