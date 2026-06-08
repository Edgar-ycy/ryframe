# RyFrame

[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/Edgar-ycy/ryframe/actions/workflows/ci.yml/badge.svg)](https://github.com/Edgar-ycy/ryframe/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/Edgar-ycy/ryframe/branch/main/graph/badge.svg)](https://codecov.io/gh/Edgar-ycy/ryframe)

**RyFrame** —— 基于 Rust + Axum 的现代化企业级后端框架。提供开箱即用的认证授权、系统管理、监控运维、定时任务、代码生成等完整能力，采用 Cargo Workspace 分层架构，各 crate 职责清晰、边界明确。

## 目录

- [特性](#特性)
- [快速开始](#快速开始)
- [项目结构](#项目结构)
- [API 端点概览](#api-端点概览)
- [配置说明](#配置说明)
- [数据库初始化](#数据库初始化)
- [认证使用](#认证使用)
- [核心功能使用指南](#核心功能使用指南)
- [运行测试](#运行测试)
- [技术栈](#技术栈)
- [许可证](#许可证)

## 特性

- **认证授权**：JWT 登录/刷新/黑名单 + RBAC 权限模型 + 数据权限 DataScope
- **系统管理**：用户/角色/权限/菜单/部门/岗位/参数/字典/通知 完整 CRUD
- **安全防护**：XSS 过滤、多层限流（全局/用户级/接口级）、防重放攻击、幂等性、安全响应头、操作日志
- **Redis 缓存**：配置/字典/菜单树/部门树 读缓存 + 写失效 + 缓存预热 + 缓存击穿保护
- **监控运维**：服务器信息、增强健康检查（DB+Redis 连通性）、DB 连接池、在线用户、缓存统计、Prometheus Metrics
- **链路追踪**：OpenTelemetry 分布式追踪（可配置采样率）
- **定时任务**：Cron 调度 + 任务管理 + 执行历史 + 内置清理任务
- **代码生成**：读取表结构自动生成 CRUD 代码
- **弹性容错**：重试（指数退避）+ 熔断器 + 降级
- **多数据源**：动态数据源注册/切换 + 读写分离
- **配置热加载**：运行时自动检测并应用配置变更
- **消息队列**：Kafka 集成 + 进程内内存队列
- **分布式锁**：Redis 分布式锁
- **事件总线**：进程内异步事件发布/订阅
- **多租户**：租户隔离（数据库级/Schema 级）+ 租户配额
- **gRPC**：Tonic 服务端/客户端
- **WebSocket**：WebSocket 连接管理与消息广播
- **对象存储**：本地 / MinIO / S3 多后端动态切换
- **文件处理**：文件上传/下载 + 图片压缩 + Excel 导入导出
- **国际化**：i18n 多语言支持（中文/英文）
- **Swagger UI**：交互式 API 文档

## 快速开始

### 环境要求

- Rust 1.85+（edition 2024）
- MySQL 8.0 或 PostgreSQL 15+
- Redis 7+（可选，未配置时自动降级到内存模式）

### 1. 克隆项目

```bash
git clone https://github.com/Edgar-ycy/ryframe.git
cd ryframe
```

### 2. 初始化数据库

```bash
# MySQL 示例
mysql -u root -p -e "CREATE DATABASE IF NOT EXISTS ryframe_config DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_general_ci;"
mysql -u root -p ryframe_config < sql/ryframe_config.sql
```

### 3. 配置连接

编辑 `config/app.dev.toml`，修改数据库连接信息：

```toml
[[database.connections]]
driver = "mysql"        # mysql / postgres / sqlite
host = "localhost"
port = 3306
database = "ryframe_config"
username = "root"
password = "your_password"
max_connections = 10
min_connections = 1
```

配置文件采用**分层覆盖**机制：`app.toml`（默认值）→ `app.{env}.toml`（环境覆盖），通过 `APP_ENV` 环境变量切换环境。

### 4. 启动服务

```bash
cargo run
```

### 5. 访问

| 入口         | 地址                                           |
|------------|----------------------------------------------|
| API 服务     | http://localhost:8080                        |
| 健康检查       | http://localhost:8080/health                 |
| Swagger UI | http://localhost:8080/api/v1/swagger-ui      |
| Prometheus | http://localhost:8080/api/v1/monitor/metrics |
| API 版本     | http://localhost:8080/api/v1/version         |

### 6. 登录测试

```bash
curl -X POST http://localhost:8080/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"123456"}'
```

### 默认账户

| 账号    | 密码     | 角色          |
|-------|--------|-------------|
| admin | 123456 | 超级管理员（全部权限） |
| user  | 123456 | 普通用户（基础权限）  |

### 环境变量

| 变量名                 | 说明                  | 默认值                               |
|---------------------|---------------------|-----------------------------------|
| `APP_ENV`           | 运行环境（dev/test/prod） | `dev`                             |
| `APP_CONFIG_DIR`    | 配置文件目录              | `config`                          |
| `OTEL_ENABLED`      | 启用链路追踪              | `false`                           |
| `OTEL_ENDPOINT`     | OTLP 端点             | `http://localhost:4318/v1/traces` |
| `OTEL_SERVICE_NAME` | 服务名称                | `ryframe`                         |
| `OTEL_SAMPLE_RATE`  | 采样率（0.0-1.0）        | `1.0`                             |

## 项目结构

```
ryframe/
├── config/              # 配置文件（dev/test/prod）
├── sql/                 # 数据库初始化脚本
├── locales/             # 国际化语言文件
├── crates/
│   ├── ryframe/         # 应用入口（main.rs / app.rs）
│   ├── ryframe-api/     # HTTP 层（Handler / DTO / Router / OpenAPI）
│   ├── ryframe-service/ # 业务逻辑层
│   ├── ryframe-db/      # 数据访问层（Entity / Repository / Migration）
│   ├── ryframe-core/    # 核心抽象（Context / 多数据源 / Redis / 缓存 / 分布式锁 /
│   │                    #   事件总线 / 消息队列 / 多租户 / 弹性容错 / gRPC / 配置热加载）
│   ├── ryframe-auth/    # 认证授权（JWT / RBAC / 密码加密 / 权限中间件）
│   ├── ryframe-config/  # 配置管理（TOML 解析 / 多环境覆盖 / 加密配置）
│   ├── ryframe-common/  # 公共工具（错误类型 / 常量 / 工具函数 / 国际化 / 枚举）
│   ├── ryframe-middleware/ # 中间件（限流 / XSS / Metrics / 请求日志 / CORS /
│   │                       #   请求体限制 / 超时 / 压缩 / 安全头 / 幂等性 /
│   │                       #   防重放 / WebSocket / 链路追踪 / 缓存控制）
│   ├── ryframe-monitor/ # 服务监控（服务器信息 / 健康检查 / 缓存统计 / DB 连接池）
│   ├── ryframe-task/    # 定时任务（Cron 调度 / 任务管理 / 内置清理任务）
│   ├── ryframe-generator/ # 代码生成器
│   └── ryframe-macro/   # 过程宏
```

### 架构分层

```
接入层（Presentation）
  ryframe-api / ryframe-middleware / ryframe-monitor
        │
领域层（Domain）
  ryframe-service / ryframe-auth
        │
基础设施层（Infrastructure）
  ryframe-db / ryframe-core / ryframe-config
        │
基础共享层（Foundation）
  ryframe-common / ryframe-task / ryframe-generator / ryframe-macro
```

- **上层可见下层，下层对上层无感知**
- 所有依赖通过构造函数注入 + `AppState` 集中管理
- 面向 trait 编程，Service 和 Repository 均可 Mock 测试

## API 端点概览

### 认证 (`/api/v1/auth`)

| 方法     | 路径                  | 说明           | 认证 |
|--------|---------------------|--------------|----|
| `POST` | `/login`            | 用户登录         | 否  |
| `POST` | `/refresh`          | 刷新令牌         | 否  |
| `POST` | `/logout`           | 用户登出         | 是  |
| `GET`  | `/me`               | 当前用户信息+菜单+角色 | 是  |
| `GET`  | `/captcha/image`    | 获取验证码图片      | 否  |
| `GET`  | `/profile`          | 个人信息         | 是  |
| `PUT`  | `/profile`          | 更新个人信息       | 是  |
| `PUT`  | `/profile/password` | 修改密码         | 是  |

### 系统管理 (`/api/v1/system`) —— 全部需要认证

| 子模块  | 路径             | 主要操作                            |
|------|----------------|---------------------------------|
| 用户管理 | `/users`       | 分页查询/详情/创建/更新/删除/重置密码/修改状态      |
| 角色管理 | `/roles`       | 列表/详情/创建/更新/删除/分配权限/分配菜单/设置数据权限 |
| 权限管理 | `/permissions` | 权限树查询                           |
| 菜单管理 | `/menus`       | 菜单树/创建/更新/删除                    |
| 部门管理 | `/depts`       | 部门树/创建/更新/删除                    |
| 岗位管理 | `/posts`       | 列表/创建/更新/删除                     |
| 参数配置 | `/configs`     | 列表/详情/按Key查询/创建/更新/删除           |
| 字典管理 | `/dict`        | 类型CRUD + 数据CRUD + 按类型获取         |
| 通知公告 | `/notices`     | 列表/创建/更新/删除                     |
| 操作日志 | `/operlogs`    | 分页查询/清空                         |
| 登录日志 | `/loginlogs`   | 分页查询/清空                         |
| 定时任务 | `/jobs`        | CRUD/暂停/恢复/立即触发                 |
| 在线用户 | `/online`      | 列表/强制踢出                         |

### 监控 (`/api/v1/monitor`)

| 路径                | 说明                   |
|-------------------|----------------------|
| `/server`         | 服务器 CPU/内存/磁盘信息      |
| `/health`         | 增强健康检查（DB+Redis 连通性） |
| `/cache`          | 缓存命中率统计              |
| `/cache/commands` | Redis 命令统计           |
| `/db-pool`        | 数据库连接池状态             |
| `/metrics`        | Prometheus 指标导出      |

### 其他

| 前缀                              | 说明                  |
|---------------------------------|---------------------|
| `/api/v1/tools/gen`             | 代码生成器（表列表/预览/生成）    |
| `/api/v1/common`                | 文件上传（公开）/ 文件下载（需认证） |
| `/api/v1/version`               | API 版本信息与端点列表       |
| `/api/v1/api-docs/openapi.json` | OpenAPI 3.0 JSON 文档 |
| `/api/v1/swagger-ui`            | Swagger UI 交互文档     |
| `/health`                       | 服务存活检测              |

### 统一响应格式

```json,ignore
{
    "code": 200,
    "message": "操作成功",
    "data": { ... }
}
```

| 状态码 | 含义           |
|-----|--------------|
| 200 | 操作成功         |
| 400 | 请求参数错误       |
| 401 | 未认证（令牌无效/过期） |
| 403 | 无权限          |
| 404 | 资源不存在        |
| 409 | 数据冲突         |
| 429 | 请求过于频繁（限流）   |
| 500 | 服务器内部错误      |

### 分页格式

请求：`GET /api/v1/system/users/list?page=1&page_size=10`

响应：
```json,ignore
{
    "code": 200,
    "data": {
        "items": [...],
        "total": 100,
        "page": 1,
        "page_size": 10,
        "total_pages": 10
    }
}
```

## 配置说明

### 配置文件结构

```toml
# config/app.toml —— 默认值（所有环境共享）
# config/app.dev.toml —— 开发环境覆盖
# config/app.prod.toml —— 生产环境覆盖
# config/app.test.toml —— 测试环境覆盖
```

### 主要配置项

| 配置节                | 说明                                 |
|--------------------|------------------------------------|
| `[app]`            | 应用名称、版本、监听地址端口                     |
| `[database]`       | SQL 日志级别 + 多数据源连接配置                |
| `[auth]`           | JWT 密钥、Token 过期时间                  |
| `[redis]`          | Redis 连接（可选，未配置时降级内存模式）            |
| `[logger]`         | 日志级别/格式（text/json）/输出（stdout/file） |
| `[cors]`           | CORS 允许的域名列表                       |
| `[rate_limit]`     | 全局限流/用户级/接口级限流参数                   |
| `[object_storage]` | 对象存储后端（local/minio/s3）及凭证          |

### 多数据源配置

```toml
# 第一个连接为主库
[[database.connections]]
driver = "mysql"
host = "localhost"
port = 3306
database = "ryframe_config"
username = "root"
password = "123456"
max_connections = 10

# 第二个为额外数据源（可做读写分离）
[[database.connections]]
driver = "mysql"
host = "localhost"
port = 3306
database = "ryframe_device"
username = "root"
password = "123456"
max_connections = 5
```

代码中可通过 `DataSourceManager` 按名称动态切换数据源。

### 环境变量注入（生产环境推荐）

```bash
export APP_ENV=prod
# 敏感信息通过 APP_ 前缀环境变量注入
export APP_DATABASE_CONNECTIONS_0_PASSWORD=prod_secret
export APP_AUTH_JWT_SECRET=your-jwt-secret
export APP_REDIS_PASSWORD=redis_password
```

## 数据库初始化

### 数据库要求

- 字符集：`utf8mb4`
- 排序规则：`utf8mb4_general_ci`
- MySQL 连接 URL 需显式指定 `collation=utf8mb4_general_ci`

### 执行初始化

```bash
# 创建数据库
mysql -u root -p -e "CREATE DATABASE IF NOT EXISTS ryframe_config \
  DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_general_ci;"

# 执行建表脚本（含默认数据）
mysql -u root -p ryframe_config < sql/ryframe_config.sql
```

### 数据表清单（18 张表）

| 表名                | 说明            |
|-------------------|---------------|
| `sys_user`        | 系统用户          |
| `sys_role`        | 系统角色          |
| `sys_permission`  | 权限资源（树形）      |
| `sys_menu`        | 系统菜单（树形）      |
| `sys_dept`        | 组织部门（树形）      |
| `sys_post`        | 岗位            |
| `sys_config`      | 系统参数配置        |
| `sys_dict_type`   | 字典类型          |
| `sys_dict_data`   | 字典数据          |
| `sys_notice`      | 通知公告          |
| `sys_oper_log`    | 操作日志          |
| `sys_login_info`  | 登录日志          |
| `sys_job`         | 定时任务          |
| `sys_job_log`     | 任务执行日志        |
| `user_role`       | 用户-角色关联       |
| `role_permission` | 角色-权限关联       |
| `role_menu`       | 角色-菜单关联       |
| `sys_role_dept`   | 角色-部门关联（数据权限） |

## 认证使用

### JWT 认证流程

```
1. POST /api/v1/auth/login  →  获取 access_token + refresh_token
2. 后续请求携带  Authorization: Bearer <access_token>
3. access_token 过期后  POST /api/v1/auth/refresh  →  刷新令牌
4. POST /api/v1/auth/logout  →  令牌加入黑名单
```

### Token 配置

```toml
[auth]
jwt_secret = "change-me-in-production"  # 生产环境务必修改
access_token_expire = "1h"              # 访问令牌有效期
refresh_token_expire = "168h"           # 刷新令牌有效期（7天）
```

### RBAC 权限模型

```
用户 (User) ──多对多──> 角色 (Role) ──多对多──> 权限 (Permission)
                                     ──多对多──> 菜单 (Menu)
```

- 超级管理员拥有 `*:*:*` 通配符权限
- 数据权限支持 5 种范围：全部/自定义/本部门/本部门及以下/仅本人

### 中间件执行顺序

```
Metrics → Telemetry → RequestId → Compression → CORS
  → RequestLog → XssFilter → Timeout → BodyLimit
  → ApiRateLimit → RateLimit → Auth → OperLog → Handler
```

## 核心功能使用指南

### 文件上传/下载

```bash
# 上传（公开）
curl -X POST http://localhost:8080/api/v1/common/upload \
  -F "file=@image.png"

# 下载（需认证）
curl http://localhost:8080/api/v1/common/download/avatar/xxx.png \
  -H "Authorization: Bearer <token>"
```

支持后端：本地文件系统 / MinIO / AWS S3。配置 `[object_storage]` 节切换后端。

### 操作日志

POST/PUT/DELETE 请求自动记录到 `sys_oper_log` 表，包含：
- 操作人、IP、URL、请求方法
- 请求参数（自动截断至 2000 字符）
- 响应结果、耗时（毫秒）
- 字段变更差异（DataDiff）

### 缓存使用

```rust,ignore
use ryframe_core::cache::{Cache, BreakdownGuard, RedisCache};

// 基础读写
cache.set("user:1", &user, 3600).await?;
let user: Option<User> = cache.get("user:1").await?;

// Get-or-Load（自动回源，防缓存击穿）
let user = cache.get_or_load("user:1", 3600, || db.find_user(1)).await?;
```

### 幂等性

防止重复提交，请求头携带唯一标识：

```
Idempotency-Key: unique-request-id-12345
```

### 重放防护

```http
X-Timestamp: 1716768000
X-Nonce: random-nonce-value
```

### gRPC 通信

```rust,ignore
use ryframe_core::grpc::{GrpcServer, GrpcClient, GrpcServerConfig, GrpcClientConfig};

// 服务端
let server = GrpcServer::new(GrpcServerConfig::default());
server.serve(my_service).await?;

// 客户端
let client = GrpcClient::connect(&GrpcClientConfig::new("http://localhost:50051")).await?;
```

### 多租户

通过请求头 `X-Tenant-Id` 识别租户，支持 `SharedTable` / `SeparateSchema` / `SeparateDatabase` 三种隔离策略。

### 消息队列

```rust,ignore
use ryframe_core::message_queue::{create_in_memory_mq, publish_json};

let mq = create_in_memory_mq();
mq.subscribe("user.created", |msg| async { Ok(()) }).await?;
publish_json(&mq, "user.created", &user_data).await?;
```

## 运行测试

```bash
# 全量测试（包括集成测试）
cargo test --workspace

# 仅单元测试
cargo test --workspace --lib

# 使用 nextest 并行测试（推荐，需先安装 cargo-nextest）
cargo nextest run --workspace

# 性能基准
cargo bench --workspace

# 代码风格检查
cargo fmt --check --all

# Clippy 检查
cargo clippy --workspace -- -D warnings

# 依赖审计
cargo audit
```

## 技术栈

| 层次     | 技术                                | 版本                               |
|--------|-----------------------------------|----------------------------------|
| Web 框架 | Axum + Tower                      | 0.8 / 0.5                        |
| 异步运行时  | Tokio                             | 1.x                              |
| ORM    | SeaORM                            | 2.0（MySQL / PostgreSQL / SQLite） |
| 认证     | JWT (jsonwebtoken) + Argon2       | 10.4 / 0.5                       |
| 权限     | RBAC + 数据权限 DataScope             | —                                |
| 缓存     | Redis (redis-rs) + 本地内存降级         | 0.27                             |
| 日志     | Tracing + Subscriber + Appender   | 0.1 / 0.3 / 0.2                  |
| 序列化    | Serde + TOML + JSON               | 1.x                              |
| 监控     | Prometheus + Sysinfo              | 0.14 / 0.39                      |
| 链路追踪   | OpenTelemetry（OTLP over HTTP）     | 0.32                             |
| API 文档 | Utoipa + Swagger UI               | 5.x                              |
| gRPC   | Tonic + Prost                     | 0.14                             |
| 消息队列   | Kafka (rdkafka) + 进程内内存队列         | 0.39                             |
| 对象存储   | 本地 / MinIO / S3（reqwest + rustls） | 0.13                             |
| 邮件     | Lettre（SMTP）                      | 0.11                             |
| 图片处理   | Image（压缩/裁剪）                      | 0.25                             |
| Excel  | Calamine（读）+ XlsxWriter（写）        | 0.34 / 0.94                      |
| 安全     | Ammonia XSS + AES-GCM + HMAC      | 4.x / 0.10 / 0.13                |
| 验证     | Validator 自动校验                    | 0.20                             |
| 任务调度   | Cron                              | 0.16                             |
| UUID   | uuid（v4 / v7）                     | 1.x                              |

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Edgar-ycy/ryframe&type=timeline&logscale&legend=top-left)](https://www.star-history.com/?repos=Edgar-ycy%2Fryframe&type=timeline&logscale=&legend=top-left)

## 许可证

MIT License
