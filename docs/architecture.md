# RyFrame 企业级Web框架 —— 系统架构设计文档

---

## 一、项目概述

RyFrame 是一个基于 **Rust** 语言开发的企业级 Web 应用框架，采用前后端分离架构。框架以 **高性能、高可扩展性、模块化** 为核心设计理念，提供数据库访问、业务逻辑处理、API 接口管理、权限认证、配置管理等完整的企业级能力，旨在支撑大型企业项目的快速开发与长期演进。

### 核心设计目标

| 目标 | 说明 |
|------|------|
| **高性能** | 基于 Tokio 异步运行时与 Axum Web 框架，充分利用 Rust 零成本抽象与内存安全特性 |
| **模块化** | 通过 Cargo Workspace 将系统拆分为多个职责清晰的 crate，支持按需组合与独立演进 |
| **可扩展** | 核心层定义 trait 抽象，业务层面向接口编程，支持灵活替换实现与功能扩展 |
| **企业级** | 内置 RBAC 权限模型、JWT 认证、多数据源支持、配置中心化等企业应用必备能力 |

---

## 二、系统架构总览

### 2.1 分层架构图

```
┌─────────────────────────────────────────────────────────────────┐
│                      前端应用 (SPA / Mobile)                       │
│                   React / Vue / Angular / Flutter                │
└──────────────────────────────┬──────────────────────────────────┘
                               │ HTTP / HTTPS (JSON)
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                 中间件管道 (Middleware Pipeline)                    │
│   Metrics → Telemetry → RequestId → Compression → CORS →        │
│   RequestLog → XssFilter → Timeout → BodyLimit → ApiRateLimit →  │
│   RateLimit → Auth → OperLog → Handler                           │
│   ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐   │
│   │ 幂等性    │  │ 重放防护  │  │ 安全头    │  │  ETag 缓存   │   │
│   └──────────┘  └──────────┘  └──────────┘  └──────────────┘   │
└──────────────────────────────┬──────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                      API 接口层 (API Layer)                       │
│                  ryframe-api                                     │
│   ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌────────────┐  │
│   │  路由管理  │  │ 请求校验  │  │ 响应序列化 │  │  DTO 提取  │  │
│   │  Router   │  │Validator  │  │ Serialize  │  │ Extractor  │  │
│   └───────────┘  └───────────┘  └───────────┘  └────────────┘  │
└──────────────────────────────┬──────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                   业务逻辑层 (Service Layer)                       │
│                  ryframe-service                                 │
│   ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌────────────┐  │
│   │ 用户服务  │  │ 角色服务  │  │ 菜单服务  │  │  认证服务   │  │
│   │UserService│  │RoleService│  │MenuService│  │ AuthService│  │
│   └───────────┘  └───────────┘  └───────────┘  └────────────┘  │
│                         │                                       │
│           依赖注入 (DI: Constructor Injection + AppState)        │
└──────────────────────────────┬──────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                  数据访问层 (Data Access Layer)                    │
│                    ryframe-db                                    │
│   ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌────────────┐  │
│   │ 连接池    │  │ 事务管理  │  │ ORM 映射  │  │  多数据源   │  │
│   │Pool Mgmt  │  │Transaction│  │ SeaORM    │  │  Routing   │  │
│   └───────────┘  └───────────┘  └───────────┘  └────────────┘  │
└──────────────────────────────┬──────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                    数据库 (Databases)                             │
│         PostgreSQL  │  MySQL  │  SQLite  │  (可扩展)             │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 模块依赖关系

框架采用 **基础共享 → 基础设施 → 领域 → 接入 → 入口** 的四层依赖模型，上层可见下层，下层对上层无感知：

```
┌─────────────────────────────────────────────────────────────────┐
│                    接入层 (Presentation Layer)                    │
│  ┌─────────────────┐ ┌─────────────────┐ ┌───────────────────┐  │
│  │   ryframe-api   │ │ryframe-middleware│ │  ryframe-monitor   │  │
│  │ Router,Handler, │ │CORS,Log,Limit,  │ │ ServerInfo,Health, │  │
│  │ DTO, Extractor  │ │XssFilter        │ │ CacheStats(可选)   │  │
│  └────────┬────────┘ └───────┬─────────┘ └─────────┬─────────┘  │
└───────────┼──────────────────┼─────────────────────┼────────────┘
            │                  │                     │
┌───────────┼──────────────────┼─────────────────────┼────────────┐
│           │          领域层 (Domain Layer)           │            │
│  ┌────────┴────────┐  ┌──────┴──────────────────────┴────────┐   │
│  │   ryframe-auth  │  │          ryframe-service              │   │
│  │ JWT,RBAC,       │  │  system::UserService                  │   │
│  │ PermissionGuard │  │  system::RoleService                  │   │
│  └────────┬────────┘  │  system::MenuService                  │   │
│           │           │  system::DeptService                  │   │
│           │           │  system::ConfigService                │   │
│           │           │  system::DictService                  │   │
│           │           │  system::NoticeService                │   │
│           │           │  monitor::LogService                  │   │
│           │           └──────────────┬───────────────────────┘   │
└───────────┼──────────────────────────┼───────────────────────────┘
            │                          │
┌───────────┼──────────────────────────┼───────────────────────────┐
│           │     基础设施层 (Infrastructure Layer)                   │
│  ┌────────┴────────┐ ┌───────────────┴──────────┐ ┌───────────┐ │
│  │   ryframe-db    │ │    ryframe-config         │ │ ryframe-  │ │
│  │ Entities        │ │ AppConfig, DbConfig       │ │   core    │ │
│  │ Repositories    │ │ AuthConfig, RedisConfig   │ │ServiceTrait│ │
│  │ ConnectionPool  │ │ EnvLoader, ProfileLoader  │ │Repo Trait │ │
│  │ TransactionMgr  │ │                           │ │Page Trait │ │
│  └────────┬────────┘ └───────────────┬───────────┘ └─────┬─────┘ │
└───────────┼──────────────────────────┼───────────────────┼───────┘
            │                          │                   │
┌───────────┴──────────────────────────┴───────────────────┴───────┐
│                   基础共享层 (Foundation Layer)                     │
│  ┌──────────────────────┐  ┌──────────────────┐                  │
│  │   ryframe-common     │  │ ryframe-task     │                  │
│  │ AppError, AppResult  │  │ Cron Scheduler   │                  │
│  │ Constants, Enums     │  │ Task Manager     │                  │
│  │ Utils, Annotations   │  │ Task History     │                  │
│  └──────────────────────┘  └──────────────────┘                  │
│  ┌──────────────────────────────────────────────┐                │
│  │        ryframe-generator                     │                │
│  │  CodeGenerator, Template Engine, TypeMapping │                │
│  └──────────────────────────────────────────────┘                │
│  ┌──────────────────────────────────────────────┐                │
│  │        ryframe-macro                         │                │
│  │  DataDiff, EnumString 派生宏                  │                │
└───────────────────────────────────────────────────────────────────┘

           ryframe (bin) —— 入口层，组装所有 crate
```

| 层级 | Crate | 可见性规则 |
|------|-------|-----------|
| **基础共享** | common, task, generator | 无上行依赖，仅依赖第三方 |
| **基础设施** | core, config, db | 仅依赖基础共享层 |
| **领域** | service, auth | 依赖基础设施层 + 基础共享层 |
| **接入** | api, middleware, monitor | 依赖领域层 + 基础设施层 |
| **入口** | ryframe (bin) | 唯一可依赖全部 crate 的位置 |
---

## 三、技术选型

| 层次 | 技术 | 版本 | 选型理由 |
|------|------|------|----------|
| **Web 框架** | [Axum](https://github.com/tokio-rs/axum) | 0.8.x | 基于 Tower + Hyper 生态，类型安全，性能卓越，与 Tokio 深度集成 |
| **异步运行时** | [Tokio](https://tokio.rs/) | 1.x | Rust 异步生态事实标准，全特性支持 |
| **ORM** | [SeaORM](https://www.sea-ql.org/SeaORM/) | 2.0.x | 异步优先，支持 PostgreSQL / MySQL / SQLite，迁移工具完善 |
| **序列化** | [Serde](https://serde.rs/) | 1.x | Rust 序列化/反序列化标准库 |
| **JWT** | [jsonwebtoken](https://github.com/Keats/jsonwebtoken) | 10.4.0 | 成熟的 JWT 编码/解码库 |
| **密码哈希** | [argon2](https://github.com/RustCrypto/password-hashes) | 0.5.x | 内存-hard 哈希算法，抗 GPU 暴力破解 |
| **配置解析** | [toml](https://github.com/toml-rs/toml) | 1.x | TOML 配置文件解析 |
| **日志** | [tracing](https://github.com/tokio-rs/tracing) | 0.1.x | 结构化日志，支持异步 span 追踪 |
| **校验** | [validator](https://github.com/Keats/validator) | 0.20 | 派生宏实现请求参数校验 |
| **UUID** | [uuid](https://github.com/uuid-rs/uuid) | 1.x | 主键 ID 生成 |

### 支持的数据库

| 数据库 | 驱动 | 适用场景 |
|--------|------|----------|
| PostgreSQL | `sea-orm` + `sqlx-postgres` | 生产环境首选，功能最丰富 |
| MySQL / MariaDB | `sea-orm` + `sqlx-mysql` | 兼容存量 MySQL 系统 |
| SQLite | `sea-orm` + `sqlx-sqlite` | 开发环境、单机部署、嵌入式场景 |

---

## 四、Cargo Workspace 与 Crate 划分

### 4.1 Workspace 配置概览

```toml
[workspace]
members = [
    "crates/ryframe",
    "crates/ryframe-api", "crates/ryframe-auth", "crates/ryframe-common", "crates/ryframe-config", "crates/ryframe-core", "crates/ryframe-db", "crates/ryframe-middleware", "crates/ryframe-service",
    "crates/ryframe-task", "crates/ryframe-generator", "crates/ryframe-monitor", "crates/ryframe-macro",
    "examples/basic",
]
resolver = "3"

[workspace.package]
version = "0.1.0"
edition = "2024"
authors = ["edgar_ye <edgar@example.com>"]
license = "MIT"
repository = ""
description = "rust开发的框架"


[workspace.dependencies]
# --- Web 框架 ---
axum = "0.8.9"
tokio = { version = "1", features = ["full"] }

# --- 数据库 ---
sea-orm = { version = "2.0.0-rc", features = [
    "sqlx-mysql", "sqlx-postgres", "sqlx-sqlite",
    "runtime-tokio-rustls", "macros", "mock",
] }

# --- 序列化 ---
serde = "1"

# --- 认证 ---
jsonwebtoken = "10.4.0"
argon2 = "0.5"

# --- 配置 ---
toml = "1"

# --- 日志与追踪 ---
tracing = "0.1"

# --- 校验 ---
validator = "0.20"

# --- 工具 ---
uuid = "1"
```

### 4.2 Crate 职责定义

| Crate | 类型 | 职责描述 |
|-------|------|----------|
| `ryframe` | **bin** | 应用入口，负责启动服务、注册模块、组装依赖 |
| `ryframe-common` | **lib** | 通用基础库：统一错误类型(`AppError`)、通用结果类型(`AppResult`)、工具函数 |
| `ryframe-config` | **lib** | 配置管理：加载并解析 TOML 配置文件，提供类型安全的配置读取接口 |
| `ryframe-core` | **lib** | 核心抽象层：定义 `Service`、`Repository` 等 trait，面向接口编程的契约 |
| `ryframe-db` | **lib** | 数据访问层：SeaORM 实体定义、连接池管理、事务封装、Repository 实现 |
| `ryframe-service` | **lib** | 业务逻辑层：实现 `core` 中定义的服务 trait，编排业务逻辑 |
| `ryframe-api` | **lib** | API 接口层：路由注册、Handler 函数、请求/响应 DTO、参数校验 |
| `ryframe-auth` | **lib** | 权限认证：JWT 生成/验证、登录认证、RBAC 权限中间件 |
| `ryframe-middleware` | **lib** | 通用中间件：CORS 配置、请求日志、限流、请求 ID 追踪、XSS 过滤 |

**可选扩展 Crate**（按需引入）：

| Crate | 类型 | 层级 | 职责描述 |
|-------|------|------|----------|
| `ryframe-task` | **lib** | 基础共享 | 定时任务模块：Cron 调度器、任务注册管理、执行历史记录 |
| `ryframe-generator` | **lib** | 基础共享 | 代码生成模块：实体/仓库/服务/Handler 模板化自动生成 |
| `ryframe-monitor` | **lib** | 接入 | 系统监控：服务器信息、健康检查、在线用户、缓存统计 |

---

## 五、项目文件结构

### 5.1 当前实际文件结构

> **实现状态**：项目已实现核心业务功能，工程化基础设施完善。以下为当前实际存在的文件结构。

```
ryframe/
│
├── Cargo.toml                          # Workspace 根配置
├── Cargo.lock                          # 依赖锁文件
├── .gitignore                          # Git 忽略规则
├── Dockerfile                          # 多阶段构建
├── docker-compose.yml                  # 本地开发环境
├── deploy.sh                           # 一键部署脚本
│
├── docs/                               # 文档目录
│   ├── architecture.md                 # 系统架构设计文档（本文件）
│   ├── api-guide.md                    # API 开发指南
│   ├── db-guide.md                     # 数据库使用指南
│   ├── deployment.md                   # 部署运维文档
│   └── large-project-roadmap.md        # 开发路线图
│
├── config/                             # 配置文件目录
│   ├── app.toml                        # 默认配置
│   ├── app.dev.toml                    # 开发环境配置
│   ├── app.prod.toml                   # 生产环境配置
│   └── app.test.toml                   # 测试环境配置
│
├── deploy/                             # 部署相关资源
│   ├── nginx.conf                      # Nginx 反向代理配置
│   ├── k8s/all-in-one.yaml             # Kubernetes 部署清单
│   ├── grafana/dashboards/             # Grafana 监控面板
│   ├── prometheus/prometheus.yml       # Prometheus 配置
│   ├── scripts/                        # 运维脚本
│   └── tests/                          # 冒烟测试 + 压测脚本
│
├── sql/                                # SQL 初始化脚本
│   └── ryframe_config.sql
│
├── locales/                            # 国际化资源
│   ├── zh-CN.toml
│   └── en-US.toml
│
└── crates/                             # Cargo Workspace 子包
    │
    ├── ryframe/                        # 主应用入口 (binary)
    │   ├── Cargo.toml
    │   └── src/
    │       ├── main.rs                 # 程序入口
    │       └── app.rs                  # 应用组装（中间件管道 + 路由注册）
    │
    ├── ryframe-common/                 # 通用基础库
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # 模块导出
    │       ├── error.rs                # AppError 统一错误类型
    │       ├── result.rs               # AppResult<T> 统一结果类型
    │       ├── constants.rs            # 全局常量
    │       ├── i18n.rs                 # 国际化支持
    │       ├── sql_log_flag.rs         # SQL 日志开关
    │       ├── enums/                  # 业务枚举
    │       ├── annotations/            # 自定义派生宏
    │       └── utils/                  # 工具函数（crypto/ip/tree/excel/string/email 等 12 个）
    │
    ├── ryframe-config/                 # 配置管理模块
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # 模块导出 + 配置加载器
    │       ├── app_config.rs           # 应用全局配置
    │       ├── db_config.rs            # 数据库连接配置
    │       ├── auth_config.rs          # JWT 认证配置
    │       ├── redis_config.rs         # Redis 缓存配置
    │       ├── logger_config.rs        # 日志配置
    │       ├── cors_config.rs          # CORS 跨域配置
    │       ├── rate_limit_config.rs    # 限流配置
    │       └── object_storage_config.rs # 对象存储配置
    │
    ├── ryframe-core/                   # 核心抽象层
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # 模块导出
    │       ├── service.rs              # Service trait 定义
    │       ├── repository.rs           # Repository + PageQuery/PageResult trait
    │       ├── context.rs              # 应用上下文
    │       ├── config_watcher.rs       # 配置热加载
    │       ├── datasource.rs           # 多数据源管理（读写分离）
    │       ├── cache.rs                # 缓存抽象（Redis/Local/Noop + 防护策略）
    │       ├── redis_client.rs         # Redis 客户端封装
    │       ├── message_queue.rs        # 消息队列抽象（Kafka/InMemory/Noop）
    │       ├── multi_tenant.rs         # 多租户（租户识别 + 数据隔离 + 配额）
    │       ├── resilience.rs           # 弹性组件（熔断器 CircuitBreaker）
    │       ├── distributed_lock.rs     # 分布式锁（Redis）
    │       ├── event_bus.rs            # 事件总线
    │       ├── feature_flag.rs         # 功能开关（Feature Flags）
    │       ├── grpc.rs                 # gRPC 服务端/客户端
    │       ├── task_queue.rs           # 异步任务队列
    │       └── token_blacklist.rs      # Token 黑名单（Redis/内存）
    │
    ├── ryframe-db/                     # 数据访问层
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # 模块导出
    │       ├── connection.rs           # 连接池管理
    │       ├── transaction.rs          # 事务封装
    │       ├── pagination.rs           # 分页工具
    │       ├── entities/               # SeaORM 实体（19 个文件）
    │       │   ├── user.rs, role.rs, permission.rs, menu.rs
    │       │   ├── dept.rs, post.rs, config.rs
    │       │   ├── dict_type.rs, dict_data.rs, notice.rs
    │       │   ├── oper_log.rs, login_info.rs
    │       │   ├── job.rs, job_log.rs
    │       │   ├── user_role.rs, role_permission.rs, role_menu.rs, role_dept.rs
    │       ├── repositories/           # Repository 实现（14 个文件）
    │       │   ├── user_repo.rs, role_repo.rs, menu_repo.rs
    │       │   ├── dept_repo.rs, post_repo.rs, config_repo.rs
    │       │   ├── dict_repo.rs, notice_repo.rs
    │       │   ├── oper_log_repo.rs, login_info_repo.rs
    │       │   ├── permission_repo.rs, job_repo.rs, job_log_repo.rs
    │       └── migration/              # 数据库迁移
    │
    ├── ryframe-service/                # 业务逻辑层
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # 模块导出
    │       └── system/                 # 系统管理领域
    │           ├── mod.rs
    │           ├── user_service.rs     # 用户服务
    │           ├── role_service.rs     # 角色服务
    │           ├── menu_service.rs     # 菜单服务
    │           ├── dept_service.rs     # 部门服务
    │           ├── post_service.rs     # 岗位服务
    │           ├── config_service.rs   # 参数配置服务
    │           ├── dict_service.rs     # 字典服务
    │           ├── notice_service.rs   # 通知公告服务
    │           ├── oper_log_service.rs # 操作日志服务
    │           ├── login_log_service.rs # 登录日志服务
    │           ├── job_service.rs      # 定时任务服务
    │           ├── online_user_service.rs # 在线用户服务
    │           └── auth_service.rs     # 认证服务
    │
    ├── ryframe-api/                    # API 接口层
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # 模块导出
    │       ├── router.rs               # 路由注册（auth/system/monitor/tools/common）
    │       ├── openapi.rs              # OpenAPI 文档定义
    │       ├── versioning.rs           # API 版本协商
    │       ├── oper_log_middleware.rs   # 操作日志记录中间件
    │       ├── handlers/               # 请求处理器（19 个文件）
    │       │   ├── auth_handler.rs     # 认证（登录/登出/刷新/me）
    │       │   ├── captcha_handler.rs  # 验证码
    │       │   ├── user_handler.rs     # 用户管理
    │       │   ├── role_handler.rs     # 角色管理
    │       │   ├── menu_handler.rs     # 菜单管理
    │       │   ├── dept_handler.rs     # 部门管理
    │       │   ├── post_handler.rs     # 岗位管理
    │       │   ├── config_handler.rs   # 参数配置
    │       │   ├── dict_handler.rs     # 字典管理
    │       │   ├── notice_handler.rs   # 通知公告
    │       │   ├── oper_log_handler.rs # 操作日志
    │       │   ├── login_log_handler.rs # 登录日志
    │       │   ├── job_handler.rs      # 定时任务
    │       │   ├── online_user_handler.rs # 在线用户
    │       │   ├── permission_handler.rs # 权限管理
    │       │   ├── profile_handler.rs  # 个人中心
    │       │   ├── generator_handler.rs # 代码生成
    │       │   └── common_handler.rs   # 通用（文件上传/下载）
    │       ├── dto/                    # 数据传输对象（15 个文件）
    │       └── extractors/             # 自定义提取器（3 个文件）
    │
    ├── ryframe-auth/                   # 权限认证模块
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # 模块导出
    │       ├── jwt.rs                  # JWT 生成/验证/刷新
    │       ├── password.rs             # 密码哈希（argon2）
    │       ├── middleware.rs           # 认证中间件
    │       ├── permission.rs           # 权限校验
    │       └── rbac.rs                 # RBAC 模型
    │
    ├── ryframe-middleware/             # 通用中间件
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # 模块导出
    │       ├── cors.rs                 # CORS 跨域
    │       ├── request_id.rs           # 请求 ID 追踪
    │       ├── request_log.rs          # 请求日志（含日志脱敏）
    │       ├── rate_limit.rs           # 多层限流（全局/用户/接口级）
    │       ├── timeout.rs              # 超时控制（30s）
    │       ├── body_limit.rs           # 请求体大小限制（10MB）
    │       ├── xss_filter.rs           # XSS 输入净化
    │       ├── security_headers.rs     # 安全响应头
    │       ├── cache_control.rs        # ETag / Cache-Control 缓存
    │       ├── idempotency.rs          # 幂等性（防重复提交）
    │       ├── replay_protection.rs    # 重放攻击防护
    │       ├── metrics.rs              # Prometheus HTTP Metrics
    │       ├── telemetry.rs            # OpenTelemetry 链路追踪
    │       └── websocket.rs            # WebSocket 支持
    │
    ├── ryframe-monitor/                # 系统监控
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # 模块导出
    │       ├── server_info.rs          # 服务器 CPU/内存/磁盘
    │       └── health_check.rs         # 健康检查端点
    │
    ├── ryframe-task/                   # 定时任务
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # 模块导出
    │       ├── scheduler.rs            # Cron 调度引擎
    │       ├── task_manager.rs         # 任务管理
    │       └── context.rs              # 任务上下文
    │
    ├── ryframe-generator/              # 代码生成器
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # 模块导出
    │       ├── engine.rs               # 生成引擎
    │       ├── template.rs             # 代码模板
    │       └── type_mapping.rs         # 类型映射
    │
    └── ryframe-macro/                  # 自定义派生宏
        ├── Cargo.toml
        └── src/
            └── lib.rs                  # DataDiff / EnumString 派生宏
```

### 5.2 新增核心模块说明

以下模块为 v0.5.0 阶段新增，详情参见本章各节。

```
crates/
│
├── ryframe/                            # 主应用入口 (binary)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs                     # 程序入口：启动服务器
│       └── app.rs                      # 应用组装：注册路由、中间件、依赖注入
│
├── ryframe-common/                     # 通用基础库
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                      # 模块导出
│       ├── error.rs                    # 统一错误类型 (AppError)
│       ├── result.rs                   # 统一结果类型 (AppResult<T>)
│       ├── constants.rs                # 全局常量（状态码、缓存键等）
│       ├── enums/                      # 业务枚举定义
│       │   ├── mod.rs
│       │   ├── user_status.rs          # 用户状态枚举
│       │   └── business_type.rs        # 业务操作类型枚举
│       ├── annotations/                # 自定义派生宏
│       │   ├── mod.rs
│       │   └── data_scope.rs           # 数据权限注解宏
│       └── utils/
│           ├── mod.rs
│           ├── string.rs               # 字符串工具函数
│           ├── crypto.rs               # 加密工具（AES、哈希等）
│           ├── ip.rs                   # IP 地址解析工具
│           ├── tree.rs                 # 树形结构构建工具
│           └── excel.rs                # Excel 导入导出工具（可选）
│
├── ryframe-config/                     # 配置管理模块
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                      # 模块导出 + ConfigLoader
│       ├── app_config.rs               # 应用全局配置结构体
│       ├── db_config.rs                # 数据库连接配置
│       ├── auth_config.rs              # JWT / OAuth 配置
│       ├── redis_config.rs             # Redis 缓存配置（可选）
│       └── logger_config.rs            # 日志配置
│
├── ryframe-core/                       # 核心抽象层
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                      # 模块导出
│       ├── service.rs                  # Service trait 定义
│       ├── repository.rs               # Repository trait 定义
│       └── context.rs                  # 应用上下文抽象
│
├── ryframe-db/                         # 数据访问层
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                      # 模块导出 + Database 结构体
│       ├── connection.rs               # 数据库连接池初始化与管理
│       ├── transaction.rs              # 事务封装工具
│       ├── pagination.rs               # 分页查询封装
│       ├── entities/                   # SeaORM 实体定义
│       │   ├── mod.rs
│       │   ├── user.rs                 # 用户实体
│       │   ├── role.rs                 # 角色实体
│       │   ├── permission.rs           # 权限实体
│       │   ├── dept.rs                 # 部门实体（树形）
│       │   ├── post.rs                 # 岗位实体
│       │   ├── menu.rs                 # 菜单实体（树形）
│       │   ├── config.rs               # 参数配置实体
│       │   ├── dict_type.rs            # 字典类型实体
│       │   ├── dict_data.rs            # 字典数据实体
│       │   ├── notice.rs               # 通知公告实体
│       │   ├── user_role.rs            # 用户-角色关联
│       │   ├── role_permission.rs      # 角色-权限关联
│       │   ├── role_menu.rs            # 角色-菜单关联
│       │   ├── oper_log.rs             # 操作日志实体
│       │   └── login_info.rs           # 登录日志实体
│       └── repositories/               # Repository 实现
│           ├── mod.rs
│           ├── user_repo.rs            # 用户数据访问
│           ├── role_repo.rs            # 角色数据访问
│           ├── permission_repo.rs      # 权限数据访问
│           ├── dept_repo.rs            # 部门数据访问
│           ├── post_repo.rs            # 岗位数据访问
│           ├── menu_repo.rs            # 菜单数据访问
│           ├── config_repo.rs          # 参数配置数据访问
│           ├── dict_repo.rs            # 字典数据访问
│           ├── notice_repo.rs          # 通知公告数据访问
│           ├── oper_log_repo.rs        # 操作日志数据访问
│           └── login_info_repo.rs      # 登录日志数据访问
│
├── ryframe-service/                    # 业务逻辑层
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                      # 模块导出
│       ├── system/                     # 系统管理领域
│       │   ├── mod.rs
│       │   ├── user_service.rs         # 用户管理服务
│       │   ├── role_service.rs         # 角色管理服务
│       │   ├── permission_service.rs   # 权限管理服务
│       │   ├── menu_service.rs         # 菜单服务
│       │   ├── dept_service.rs         # 部门服务
│       │   ├── post_service.rs         # 岗位服务
│       │   ├── config_service.rs       # 参数配置服务
│       │   ├── dict_service.rs         # 字典数据服务
│       │   └── notice_service.rs       # 通知公告服务
│       ├── auth_service.rs             # 认证服务
│       └── monitor/                    # 监控管理领域
│           ├── mod.rs
│           ├── log_service.rs          # 操作日志/登录日志服务
│           └── online_user_service.rs  # 在线用户管理服务
│
├── ryframe-api/                        # API 接口层 (lib)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                      # 模块导出
│       ├── router.rs                   # 路由树注册
│       ├── app_state.rs                # 应用共享状态（AppState）
│       ├── handlers/                   # 请求处理器
│       │   ├── mod.rs
│       │   ├── auth_handler.rs         # 认证接口
│       │   ├── user_handler.rs         # 用户管理接口
│       │   ├── role_handler.rs         # 角色管理接口
│       │   ├── menu_handler.rs         # 菜单管理接口
│       │   ├── dept_handler.rs         # 部门管理接口
│       │   ├── post_handler.rs         # 岗位管理接口
│       │   ├── config_handler.rs       # 参数配置接口
│       │   ├── dict_handler.rs         # 字典管理接口
│       │   ├── notice_handler.rs       # 通知公告接口
│       │   ├── monitor_handler.rs      # 监控管理接口
│       │   └── common_handler.rs       # 通用接口（文件上传等）
│       ├── dto/                        # 数据传输对象
│       │   ├── mod.rs
│       │   ├── auth_dto.rs             # 认证 DTO
│       │   ├── user_dto.rs             # 用户 DTO
│       │   ├── role_dto.rs             # 角色 DTO
│       │   ├── menu_dto.rs             # 菜单 DTO
│       │   ├── dept_dto.rs             # 部门 DTO
│       │   ├── post_dto.rs             # 岗位 DTO
│       │   ├── config_dto.rs           # 参数配置 DTO
│       │   ├── dict_dto.rs             # 字典 DTO
│       │   ├── notice_dto.rs           # 通知公告 DTO
│       │   ├── monitor_dto.rs          # 监控 DTO
│       │   └── common_dto.rs           # 通用 DTO（分页等）
│       └── extractors/                 # 自定义提取器
│           ├── mod.rs
│           └── auth_extractor.rs       # 当前用户提取器
│
├── ryframe-auth/                       # 权限认证模块
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                      # 模块导出
│       ├── jwt.rs                      # JWT 令牌生成/验证/刷新
│       ├── password.rs                 # 密码哈希/校验
│       ├── middleware.rs               # Axum 认证中间件
│       ├── permission.rs               # 权限注解与校验逻辑
│       └── rbac.rs                     # RBAC 模型实现
│
└── ryframe-middleware/                 # 通用中间件
    ├── Cargo.toml
    └── src/
        ├── lib.rs                      # 模块导出
        ├── cors.rs                     # CORS 跨域中间件
        ├── request_id.rs               # 请求 ID 追踪
        ├── request_log.rs              # 请求日志记录
        ├── rate_limit.rs               # 限流中间件
        └── xss_filter.rs               # XSS 过滤中间件

# === 可选扩展 crate 目标结构 ===

├── ryframe-task/                       # 定时任务模块（可选）
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                      # 模块导出
│       ├── scheduler.rs                # Cron 调度引擎
│       ├── task_manager.rs             # 任务注册/暂停/恢复/删除
│       ├── task_history.rs             # 执行历史记录
│       └── builtin/                    # 内置任务
│           ├── mod.rs
│           ├── clean_log_task.rs       # 日志清理任务
│           └── db_backup_task.rs       # 数据库备份任务
│
├── ryframe-generator/                  # 代码生成模块（可选）
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                      # 模块导出
│       ├── engine.rs                   # 代码生成引擎
│       ├── template/                   # 代码模板
│       │   ├── mod.rs
│       │   ├── entity_template.rs      # 实体模板
│       │   ├── repo_template.rs        # Repository 模板
│       │   ├── service_template.rs     # Service 模板
│       │   └── handler_template.rs     # Handler 模板
│       └── type_mapping.rs             # 数据库类型 → Rust 类型映射
│
└── ryframe-monitor/                    # 系统监控模块（可选）
    ├── Cargo.toml
    └── src/
        ├── lib.rs                      # 模块导出
        ├── server_info.rs              # 服务器 CPU/内存/磁盘信息
        ├── health_check.rs             # 应用健康检查端点
        ├── cache_stats.rs              # 缓存命中率统计
        └── online_user.rs              # 在线用户统计
```

---

## 六、核心模块详细设计

### 6.1 数据库访问层 (`ryframe-db`)

#### 6.1.1 设计目标

- 支持 PostgreSQL、MySQL、SQLite 三种数据库引擎
- 连接池自动管理（基于 SeaORM 内置连接池）
- 声明式事务管理
- 支持读写分离与多数据源动态路由
- Repository 模式封装，业务层不直接操作 ORM

#### 6.1.2 连接池管理

```
┌──────────────────────────────────────────┐
│              Database Pool                │
│                                           │
│  ┌──────────┐  ┌──────────┐  ┌─────────┐ │
│  │  Primary  │  │ Replica 1│  │Replica 2│ │
│  │  (Write)  │  │ (Read)   │  │ (Read)  │ │
│  └──────────┘  └──────────┘  └─────────┘ │
│                                           │
│  Pool Config:                             │
│  - max_connections: 32                    │
│  - min_connections: 5                     │
│  - connect_timeout: 5s                    │
│  - idle_timeout: 10min                    │
└──────────────────────────────────────────┘
```

- **连接池初始化**：应用启动时读取 `db_config`，创建 `DatabaseConnection` 并注入到 `AppState`
- **多数据源**：通过 `DataSourceKey` 枚举区分主库/从库，Repository 层根据操作类型（读/写）自动路由
- **健康检查**：定期 `ping()` 数据库，自动剔除不健康连接

#### 6.1.3 事务管理

采用 **闭包式事务 API**（类似 Spring 的 `@Transactional` 概念），避免手动 begin/commit/rollback：

```
Transaction::run(&db, |tx| async {
    user_repo.create(&tx, new_user).await?;
    role_repo.assign_role(&tx, user_id, role_id).await?;
    Ok(())
}).await?;
```

- 闭包正常返回 `Ok` → 自动提交
- 闭包返回 `Err` 或 panic → 自动回滚
- 支持事务嵌套（Savepoint）

#### 6.1.4 Repository 模式

```
┌─────────────────────────────────────┐
│      Service Layer                  │
│  (依赖 trait, 非具体实现)            │
│                                     │
│  #[async_trait]                     │
│  trait UserRepository {             │
│    async fn find_by_id(id) -> User; │
│    async fn create(user) -> User;   │
│  }                                  │
└──────────────┬──────────────────────┘
               │ 面向接口编程
               ▼
┌─────────────────────────────────────┐
│     Repository Implementation       │
│  (SeaORM 具体实现)                   │
│                                     │
│  impl UserRepository                │
│    for SeaOrmUserRepository { ... } │
│                                     │
│  使用 Entity::find()                 │
│      Entity::insert()               │
└─────────────────────────────────────┘
```

**核心实体设计（ER 概要）**：

```
┌────────────┐     ┌──────────────┐     ┌──────────┐     ┌───────────────┐
│  sys_user  │     │  user_role   │     │ sys_role │     │ role_permission│
├────────────┤     ├──────────────┤     ├──────────┤     ├───────────────┤
│ id (PK)    │────→│ user_id (FK) │←────│ id (PK)  │────→│ role_id (FK)  │
│ username   │     │ role_id (FK) │     │ name     │     │ perm_id (FK)  │
│ password   │     └──────────────┘     │ code     │     └───────┬───────┘
│ nickname   │                          │ status   │             │
│ email      │                          │ sort     │             │
│ phone      │                          └──────────┘             │
│ avatar     │                                          ┌────────┴────────┐
│ status     │     ┌──────────────┐     ┌──────────┐    │  sys_permission │
│ dept_id ───┼────→│  sys_dept    │     │ sys_post │    ├────────────────┤
│ remark     │     ├──────────────┤     ├──────────┤    │ id (PK)         │
│ login_ip   │     │ id (PK)      │     │ id (PK)  │    │ name            │
│ login_date │     │ name         │     │ name     │    │ code            │
│ created_at │     │ parent_id    │     │ code     │    │ parent_id       │
│ updated_at │     │ sort         │     │ sort     │    │ type (menu/api) │
└────────────┘     │ status       │     │ status   │    │ path            │
                   └──────────────┘     └──────────┘    │ icon            │
                                                         │ sort            │
┌────────────┐     ┌──────────────┐                      │ status          │
│ sys_menu   │     │ role_menu    │                      └────────────────┘
├────────────┤     ├──────────────┤
│ id (PK)    │────→│ role_id (FK) │      ┌──────────────┐
│ name       │     │ menu_id (FK) │      │ sys_config   │
│ parent_id  │     └──────────────┘      ├──────────────┤
│ path       │                           │ id (PK)      │
│ component  │      ┌──────────────┐      │ name         │
│ icon       │      │ sys_dict_type│      │ key          │
│ sort       │      ├──────────────┤      │ value        │
│ visible    │      │ id (PK)      │      │ remark       │
│ status     │      │ name         │      └──────────────┘
└────────────┘      │ code         │
                    │ status       │      ┌──────────────┐
                    └──────┬───────┘      │ sys_dict_data│
                           │              ├──────────────┤
                    ┌──────┴───────┐      │ id (PK)      │
                    │sys_dict_data │      │ type_code (FK)│
                    ├──────────────┤      │ label        │
                    │...           │      │ value        │
                    └──────────────┘      │ sort         │
                                          │ status       │
┌──────────────┐                          │ css_class    │
│ sys_notice   │                          └──────────────┘
├──────────────┤
│ id (PK)      │      ┌──────────────┐      ┌──────────────┐
│ title        │      │sys_oper_log  │      │sys_login_info│
│ content      │      ├──────────────┤      ├──────────────┤
│ type         │      │ id (PK)      │      │ id (PK)      │
│ status       │      │ title        │      │ username     │
│ created_by   │      │ business_type│      │ ip_address   │
│ created_at   │      │ method       │      │ login_time   │
└──────────────┘      │ operator     │      │ status       │
                      │ dept_name    │      │ message      │
                      │ request_url  │      └──────────────┘
                      │ request_args │
                      │ response     │
                      │ cost_time    │
                      │ created_at   │
                      └──────────────┘
```

**实体清单与归属模块**：

| 表名 | 实体文件 | 所属领域 | 说明 |
|------|----------|----------|------|
| `sys_user` | `entities/user.rs` | system | 系统用户 |
| `sys_role` | `entities/role.rs` | system | 系统角色 |
| `sys_permission` | `entities/permission.rs` | system | 权限资源（菜单+接口） |
| `sys_dept` | `entities/dept.rs` | system | 组织部门（树形） |
| `sys_post` | `entities/post.rs` | system | 岗位 |
| `sys_menu` | `entities/menu.rs` | system | 系统菜单（树形） |
| `sys_config` | `entities/config.rs` | system | 系统参数配置 |
| `sys_dict_type` | `entities/dict_type.rs` | system | 字典类型 |
| `sys_dict_data` | `entities/dict_data.rs` | system | 字典数据 |
| `sys_notice` | `entities/notice.rs` | system | 通知公告 |
| `user_role` | `entities/user_role.rs` | system | 用户-角色关联 |
| `role_permission` | `entities/role_permission.rs` | system | 角色-权限关联 |
| `role_menu` | `entities/role_menu.rs` | system | 角色-菜单关联 |
| `sys_oper_log` | `entities/oper_log.rs` | monitor | 操作日志 |
| `sys_login_info` | `entities/login_info.rs` | monitor | 登录日志 |

---

### 6.2 业务逻辑层 (`ryframe-service`)

#### 6.2.1 设计原则

- **面向接口编程**：每个 Service 对应 `ryframe-core` 中定义的 trait，便于单元测试 Mock
- **单一职责**：一个 Service 只负责一个业务领域
- **依赖注入**：通过构造函数注入依赖（Repository trait 对象、其他 Service trait 对象）

#### 6.2.2 依赖注入方案

不使用重量级 DI 容器框架，采用 **手动构造函数注入 + AppState 集中管理** 的方式，保持 Rust 的显式与零成本抽象：

```
┌──────────────────────────────────────────────┐
│                 AppState                      │
│                                               │
│  db: DatabaseConnection                       │
│  config: AppConfig                            │
│  services: ServiceRegistry                    │
│    ├── user_service: Arc<UserServiceImpl>     │
│    ├── role_service: Arc<RoleServiceImpl>     │
│    ├── auth_service: Arc<AuthServiceImpl>     │
│    └── menu_service: Arc<MenuServiceImpl>     │
└──────────────────────────────────────────────┘
```

**注入流程**：

1. `main.rs` 启动时初始化各组件
2. 将 Repository 注入 Service
3. 将 Service 注入 Handler（通过 `AppState`）
4. Handler 通过 `State(app_state)` 提取器获取依赖

#### 6.2.3 Service 设计示例

```
UserService trait (定义于 ryframe-core):
  - find_by_id(id: Uuid) → AppResult<UserVo>
  - find_by_page(query: UserPageQuery) → AppResult<PageResult<UserVo>>
  - create(dto: CreateUserDto) → AppResult<UserVo>
  - update(id: Uuid, dto: UpdateUserDto) → AppResult<UserVo>
  - delete(id: Uuid) → AppResult<()>
  - reset_password(id: Uuid, password: String) → AppResult<()>
  - assign_roles(user_id: Uuid, role_ids: Vec<Uuid>) → AppResult<()>
```

---

### 6.3 API 接口层 (`ryframe-api`)

#### 6.3.1 RESTful API 设计规范

**认证接口**（公开）：

| 方法 | 路径 | 说明 |
|------|------|------|
| `POST` | `/api/v1/auth/login` | 用户登录 |
| `POST` | `/api/v1/auth/logout` | 用户登出 |
| `POST` | `/api/v1/auth/refresh` | 刷新令牌 |
| `GET` | `/api/v1/auth/me` | 获取当前用户信息 |
| `GET` | `/api/v1/auth/menus` | 获取当前用户菜单树 |

**系统管理接口**（需认证+权限）：

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/v1/system/users` | 分页查询用户列表 |
| `GET` | `/api/v1/system/users/:id` | 查询单个用户 |
| `POST` | `/api/v1/system/users` | 创建用户 |
| `PUT` | `/api/v1/system/users/:id` | 更新用户 |
| `DELETE` | `/api/v1/system/users/:id` | 删除用户 |
| `PUT` | `/api/v1/system/users/:id/password` | 重置用户密码 |
| `GET` | `/api/v1/system/roles` | 角色列表 |
| `POST` | `/api/v1/system/roles` | 创建角色 |
| `PUT` | `/api/v1/system/roles/:id` | 更新角色 |
| `DELETE` | `/api/v1/system/roles/:id` | 删除角色 |
| `GET` | `/api/v1/system/permissions` | 权限树 |
| `GET` | `/api/v1/system/menus` | 菜单树 |
| `POST` | `/api/v1/system/menus` | 创建菜单 |
| `PUT` | `/api/v1/system/menus/:id` | 更新菜单 |
| `DELETE` | `/api/v1/system/menus/:id` | 删除菜单 |
| `GET` | `/api/v1/system/depts` | 部门树 |
| `POST` | `/api/v1/system/depts` | 创建部门 |
| `PUT` | `/api/v1/system/depts/:id` | 更新部门 |
| `DELETE` | `/api/v1/system/depts/:id` | 删除部门 |
| `GET` | `/api/v1/system/posts` | 岗位列表 |
| `POST` | `/api/v1/system/posts` | 创建岗位 |
| `PUT` | `/api/v1/system/posts/:id` | 更新岗位 |
| `DELETE` | `/api/v1/system/posts/:id` | 删除岗位 |
| `GET` | `/api/v1/system/configs` | 参数配置列表 |
| `GET` | `/api/v1/system/configs/:id` | 查询参数配置 |
| `PUT` | `/api/v1/system/configs/:id` | 更新参数配置 |
| `GET` | `/api/v1/system/dict/types` | 字典类型列表 |
| `GET` | `/api/v1/system/dict/data/:type` | 按类型获取字典数据 |
| `GET` | `/api/v1/system/notices` | 通知公告列表 |
| `POST` | `/api/v1/system/notices` | 创建通知公告 |

**监控管理接口**（需认证+权限）：

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/v1/monitor/server` | 服务器信息 |
| `GET` | `/api/v1/monitor/health` | 应用健康检查 |
| `GET` | `/api/v1/monitor/logs/oper` | 操作日志分页查询 |
| `DELETE` | `/api/v1/monitor/logs/oper` | 清空操作日志 |
| `GET` | `/api/v1/monitor/logs/login` | 登录日志分页查询 |
| `GET` | `/api/v1/monitor/online` | 在线用户列表 |

**定时任务接口**（可选模块，需认证+权限）：

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/v1/task/list` | 任务列表 |
| `POST` | `/api/v1/task` | 创建任务 |
| `PUT` | `/api/v1/task/:id` | 更新任务 |
| `DELETE` | `/api/v1/task/:id` | 删除任务 |
| `POST` | `/api/v1/task/:id/run` | 立即执行一次 |
| `POST` | `/api/v1/task/:id/pause` | 暂停任务 |
| `POST` | `/api/v1/task/:id/resume` | 恢复任务 |

**代码生成接口**（可选模块，需认证+权限）：

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/v1/generator/tables` | 数据库表列表 |
| `GET` | `/api/v1/generator/table/:name` | 查看表结构详情 |
| `POST` | `/api/v1/generator/preview` | 预览生成代码 |
| `POST` | `/api/v1/generator/generate` | 执行代码生成 |

#### 6.3.2 统一响应格式

```json
{
  "code": 200,
  "message": "操作成功",
  "data": { ... },
  "timestamp": 1700000000000
}
```

**响应码规范**：

| 范围 | 含义 |
|------|------|
| 200 | 操作成功 |
| 400 | 请求参数错误 |
| 401 | 未认证（令牌无效/过期） |
| 403 | 无权限访问 |
| 404 | 资源不存在 |
| 409 | 数据冲突（如唯一键重复） |
| 500 | 服务器内部错误 |

#### 6.3.3 路由组织

```
/api/v1
├── /auth/*              → auth_router        (公开)
├── /system/             → system_router      (认证 + 权限)
│   ├── /users/*         → user_router
│   ├── /roles/*         → role_router
│   ├── /permissions/*   → permission_router
│   ├── /menus/*         → menu_router
│   ├── /depts/*         → dept_router
│   ├── /posts/*         → post_router
│   ├── /configs/*       → config_router
│   ├── /dict/*          → dict_router
│   └── /notices/*       → notice_router
├── /monitor/            → monitor_router     (认证 + 权限)
│   ├── /server          → server_info
│   ├── /health          → health_check
│   ├── /logs/*          → log_router
│   └── /online          → online_user
├── /task/               → task_router        (认证 + 权限, 可选)
└── /generator/          → generator_router   (认证 + 权限, 可选)
```

每个业务模块通过 `Router::new().nest()` 挂载到主路由，支持路由前缀隔离。

#### 6.3.4 请求处理流程

```
HTTP Request
    │
    ▼
┌──────────────┐
│  中间件管道   │  ← CORS, RequestId, Logging, RateLimit
└──────┬───────┘
       │
       ▼
┌──────────────┐
│  认证中间件   │  ← JWT 验证 (公开路由跳过)
└──────┬───────┘
       │
       ▼
┌──────────────┐
│  权限中间件   │  ← 权限码校验 (公开路由跳过)
└──────┬───────┘
       │
       ▼
┌──────────────┐
│  Handler     │  ← JSON 反序列化 → 参数校验 → 调用 Service → 序列化响应
└──────┬───────┘
       │
       ▼
  HTTP Response (JSON)
```

---

### 6.4 权限认证模块 (`ryframe-auth`)

#### 6.4.1 RBAC 权限模型

```
┌──────────┐       ┌──────────────┐       ┌──────────┐
│   User   │───N:M──│     Role     │───N:M──│Permission│
└──────────┘       └──────────────┘       └──────────┘
                                                    │
                                           ┌────────┴────────┐
                                           ▼                 ▼
                                     ┌──────────┐     ┌──────────┐
                                     │   Menu   │     │   API    │
                                     │Permission│     │Permission│
                                     └──────────┘     └──────────┘
```

- **用户 → 角色**：多对多（一个用户可有多个角色）
- **角色 → 权限**：多对多（一个角色关联多个权限码）
- **权限码格式**：`模块:操作`，如 `system:user:list`、`system:user:create`

#### 6.4.2 JWT 令牌设计

**令牌结构**：

```
Access Token (短期):
{
  "sub": "user_uuid",
  "username": "admin",
  "roles": ["admin", "user"],
  "perms": ["system:user:*", "system:role:list"],
  "iat": 1700000000,
  "exp": 1700003600    // 1 小时后过期
}

Refresh Token (长期):
{
  "sub": "user_uuid",
  "type": "refresh",
  "exp": 1700086400    // 7 天后过期
}
```

**令牌流转**：

```
客户端                          服务端
  │                               │
  │──── POST /auth/login ────────→│  验证用户名密码
  │                               │  返回 { access_token, refresh_token }
  │←── { tokens } ───────────────│
  │                               │
  │──── API 请求 (Authorization)─→│  验证 access_token
  │                               │
  │  (access_token 过期)          │
  │──── POST /auth/refresh ──────→│  验证 refresh_token
  │←── { new access_token } ─────│  返回新 access_token
```

#### 6.4.3 权限中间件

通过 Axum 中间件 + 自定义属性宏实现声明式权限校验：

```
// Handler 上声明所需权限
#[permission("system:user:delete")]
async fn delete_user(...) -> AppResult<Json<...>> { ... }

// 中间件流程:
// 1. 从请求头提取 JWT
// 2. 验证令牌有效性
// 3. 从令牌载荷中提取用户权限列表
// 4. 与 Handler 声明的权限码匹配
// 5. 不匹配则返回 403 Forbidden
```

---

### 6.5 配置管理模块 (`ryframe-config`)

#### 6.5.1 配置加载策略

```
加载优先级（高→低）:
  1. 环境变量 (APP_* 前缀)
  2. config/app.{env}.toml (环境特定配置)
  3. config/app.toml (默认配置)
```

**加载流程**：

```
程序启动
  │
  ▼
读取 APP_ENV 环境变量 (dev / test / prod)
  │
  ▼
加载 config/app.toml (默认值)
  │
  ▼
加载 config/app.{env}.toml (覆盖默认值)
  │
  ▼
加载环境变量覆盖 (APP_DB_HOST → db.host)
  │
  ▼
反序列化为 AppConfig 结构体
  │
  ▼
验证配置完整性 → 成功启动 / 失败退出
```

#### 6.5.2 配置结构体设计

```
AppConfig
├── app: AppSettings           # 应用基础配置
│   ├── name: String           # 应用名称
│   ├── version: String        # 版本号
│   ├── host: String           # 监听地址 (0.0.0.0)
│   └── port: u16              # 监听端口 (8080)
│
├── database: DatabaseConfig   # 数据库配置
│   ├── primary: DbConnection  # 主库连接
│   └── replicas: Vec<DbConnection>  # 从库连接（可选）
│
├── auth: AuthConfig           # 认证配置
│   ├── jwt_secret: String     # JWT 签名密钥
│   ├── access_token_expire: Duration
│   └── refresh_token_expire: Duration
│
├── redis: Option<RedisConfig> # Redis 缓存（可选）
│
└── logger: LoggerConfig       # 日志配置
    ├── level: String          # 日志级别
    ├── format: String         # 输出格式 (json / text)
    └── output: String         # 输出目标 (stdout / file)
```

#### 6.5.3 配置文件示例

`config/app.toml` (默认配置)：
```toml
[app]
name = "ryframe"
version = "0.1.0"
host = "0.0.0.0"
port = 8080

[database.primary]
driver = "postgres"          # postgres / mysql / sqlite
host = "localhost"
port = 5432
database = "ryframe"
username = "postgres"
password = ""
max_connections = 32
min_connections = 5

[auth]
jwt_secret = "change-me-in-production"
access_token_expire = "1h"
refresh_token_expire = "168h"  # 7 days

[logger]
level = "info"
format = "text"
output = "stdout"
```

---

### 6.6 中间件模块 (`ryframe-middleware`)

项目当前已实现 **15 个中间件**，覆盖安全、监控、性能、可靠性等维度：

| 中间件 | 文件 | 功能 | 实现方式 |
|--------|------|------|----------|
| **Metrics** | `metrics.rs` | Prometheus HTTP 请求指标采集 | 自定义中间件 + `prometheus` crate |
| **Telemetry** | `telemetry.rs` | OpenTelemetry 链路追踪（Span 创建） | `opentelemetry` + `tracing-opentelemetry` |
| **RequestId** | `request_id.rs` | 为每个请求生成唯一 trace_id | UUID v7 |
| **Compression** | `lib.rs` | 响应体 Gzip/Brotli 压缩 | `tower-http::compression` |
| **CORS** | `cors.rs` | 跨域请求控制（可配置 Origins） | `tower-http::cors::CorsLayer` |
| **RequestLog** | `request_log.rs` | 请求日志（方法/路径/状态码/耗时 + 日志脱敏） | `tracing` |
| **XSS 过滤** | `xss_filter.rs` | XSS 攻击输入净化 | `ammonia` crate |
| **Timeout** | `timeout.rs` | 请求超时控制（30 秒） | `tower::timeout` |
| **BodyLimit** | `body_limit.rs` | 请求体大小限制（10 MB） | 自定义中间件 |
| **ApiRateLimit** | `rate_limit.rs` | 接口级限流（如登录接口 5 次/分钟） | 固定窗口 + 内存 |
| **RateLimit** | `rate_limit.rs` | 全局限流（令牌桶/固定窗口，支持用户级） | 令牌桶 + 内存/Redis |
| **SecurityHeaders** | `security_headers.rs` | 安全响应头（CSP/HSTS/X-Frame 等） | 自定义中间件 |
| **CacheControl** | `cache_control.rs` | ETag 生成 + Cache-Control 头（304 响应） | 自定义中间件 |
| **Idempotency** | `idempotency.rs` | 幂等性（防重复提交，基于 Redis） | `Idempotency-Key` 请求头 |
| **ReplayProtection** | `replay_protection.rs` | 重放攻击防护（Timestamp + Nonce） | `X-Timestamp` + `X-Nonce` 请求头 |
| **WebSocket** | `websocket.rs` | WebSocket 连接升级支持 | Axum WebSocket |

**实际中间件注册顺序**（`app.rs`，从外到内：Layer 从下往上注册，最先注册的最外层先执行）：

```
RateLimit → ApiRateLimit → BodyLimit(10MB) → Timeout(30s) → XssFilter →
  RequestLog → CORS → Compression → RequestId → Telemetry → Metrics → Router
```

业务路由内部还有认证/授权中间件链：

```
Auth → OnlineUserTracking → OperLog → Handler
```

---

### 6.7 缓存模块 (`ryframe-core::cache`) ✅ 已实现

#### 6.7.1 设计目标

提供统一的缓存抽象层，支持多种后端实现，内置缓存防护策略（防穿透/击穿/雪崩）。

#### 6.7.2 Cache Trait

```rust
#[async_trait]
pub trait Cache: Send + Sync {
    async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, CacheError>;
    async fn set<T: Serialize>(&self, key: &str, value: &T, ttl_secs: u64) -> Result<(), CacheError>;
    async fn delete(&self, key: &str) -> Result<(), CacheError>;
    async fn exists(&self, key: &str) -> Result<bool, CacheError>;
    async fn keys(&self, prefix: &str) -> Result<Vec<String>, CacheError>;
    // get_or_load: 缓存未命中时自动回源加载
    async fn get_or_load<T, F, Fut>(&self, key: &str, ttl_secs: u64, loader: F) -> Result<T, CacheError>;
}
```

#### 6.7.3 后端实现

| 实现 | 类 | 适用场景 |
|------|-----|----------|
| **NoopCache** | 空操作 | 禁用缓存 |
| **LocalMemoryCache** | 本地 HashMap | 单机部署、开发测试 |
| **RedisCache** | Redis 封装 | 分布式生产环境 |

#### 6.7.4 缓存防护体系

| 组件 | 防护目标 | 实现方式 |
|------|----------|----------|
| **CacheStrategy** | 防穿透 + 防雪崩 | 空值缓存 + 随机 TTL 抖动 |
| **BreakdownGuard** | 防击穿 | 互斥锁 + 双检锁（Double-Check Locking） |
| **CacheWarmer** | 冷启动 | 启动时预加载热点数据 |

---

### 6.8 消息队列模块 (`ryframe-core::message_queue`) ✅ 已实现

#### 6.8.1 MessageQueue Trait

```rust
#[async_trait]
pub trait MessageQueue: Send + Sync {
    async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), MqError>;
    async fn subscribe<F, Fut>(&self, topic: &str, handler: F) -> Result<(), MqError>;
    async fn health_check(&self) -> bool;
}
```

#### 6.8.2 MqBackend 枚举（委托模式）

| Variant | 说明 | Feature |
|---------|------|---------|
| `Noop(NoopMessageQueue)` | 空操作，默认关闭 | 默认 |
| `InMemory(InMemoryMessageQueue)` | tokio broadcast 内存队列 | 默认 |
| `Kafka(KafkaMessageQueue)` | rdkafka 生产级队列 | `kafka` feature |

---

### 6.9 多租户模块 (`ryframe-core::multi_tenant`) ✅ 已实现

#### 6.9.1 租户识别

支持三种提取方式：

| 方式 | 配置 | 示例 |
|------|------|------|
| **Header** | `ExtractionMethod::Header("X-Tenant-Id")` | `curl -H "X-Tenant-Id: corp-a"` |
| **子域名** | `ExtractionMethod::Subdomain` | `corp-a.example.com` |
| **路径前缀** | `ExtractionMethod::PathPrefix` | `/api/corp-a/users` |

#### 6.9.2 数据隔离策略

| 策略 | 说明 |
|------|------|
| `SharedTable` | 共享表 + tenant_id 列过滤 |
| `DatabasePerTenant` | 独立数据库 |
| `SchemaPerTenant` | PostgreSQL Schema 隔离 |

#### 6.9.3 TenantFilter<T>

包装任意仓库实例，在查询时自动注入 `tenant_id` 过滤条件（管理员模式可跨租户）。

#### 6.9.4 租户配额

`TenantQuota` 支持限制：最大用户数、最大角色数、最大存储容量 (MB)、最大 API 请求数/分钟。

---

### 6.10 弹性组件 (`ryframe-core::resilience`) ✅ 已实现

**熔断器 (CircuitBreaker)**：三态模型（Closed → Open → HalfOpen），失败阈值 + 冷却时间自动切换。

---

### 6.11 定时任务模块 (`ryframe-task`)

#### 6.11.1 设计目标

- 支持 Cron 表达式调度（秒级精度）
- 任务可动态注册、暂停、恢复、删除
- 记录每次执行历史（状态、耗时、输出日志）
- 内置常用系统任务（日志清理、临时文件清理）
- 与 Tokio 异步运行时深度集成，不阻塞主线程

#### 6.11.2 任务调度架构

```
┌────────────────────────────────────────────┐
│              TaskScheduler                  │
│                                             │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  │
│  │ Cron     │  │ Interval │  │ One-shot │  │
│  │ "0/30 *  │  │ every    │  │ run_at   │  │
│  │  * * *"  │  │ 5min     │  │ 3:00 AM  │  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  │
│       │             │             │         │
│       └─────────────┼─────────────┘         │
│                     ▼                       │
│            ┌──────────────┐                │
│            │  Task Queue   │                │
│            │  (tokio::sync │                │
│            │   ::mpsc)     │                │
│            └──────┬───────┘                │
│                   ▼                         │
│            ┌──────────────┐                │
│            │ Task Worker   │ (spawn)        │
│            │ ├─ execute()  │                │
│            │ └─ log_result │                │
│            └──────────────┘                │
└────────────────────────────────────────────┘
```

#### 6.11.3 任务定义示例

```
#[async_trait]
pub trait ScheduledTask: Send + Sync {
    /// 任务唯一标识
    fn name(&self) -> &str;
    /// Cron 表达式
    fn cron(&self) -> &str;
    /// 执行逻辑
    async fn execute(&self, ctx: &TaskContext) -> AppResult<String>;
}
```

**内置任务清单**：

| 任务 | Cron | 说明 |
|------|------|------|
| `clean_oper_log` | `0 0 2 * * *` | 每天凌晨2点清理30天前的操作日志 |
| `clean_login_log` | `0 0 3 * * *` | 每天凌晨3点清理90天前的登录日志 |
| `clean_temp_files` | `0 30 * * * *` | 每小时清理临时上传文件 |

---

### 6.12 代码生成模块 (`ryframe-generator`)

#### 6.12.1 设计目标

- 读取数据库表结构元数据，自动生成 Entity / Repository / Service / Handler 代码
- 支持模板自定义，满足不同项目编码风格
- 生成代码遵循框架分层规范，可直接编译运行

#### 6.12.2 生成流程

```
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│ 1. 读取表结构 │───→│ 2. 解析元数据 │───→│ 3. 匹配模板   │
│   INFORMATION │    │ ColumnInfo   │    │ EntityTpl    │
│   _SCHEMA     │    │ PrimaryKey   │    │ RepoTpl      │
│              │    │ ForeignKey   │    │ ServiceTpl   │
└──────────────┘    └──────────────┘    │ HandlerTpl   │
                                        └──────┬───────┘
                                               │
                    ┌──────────────────────────┘
                    ▼
┌──────────────────────────────────────────────────┐
│ 4. 渲染 + 写入文件                                 │
│    crates/ryframe-db/src/entities/{table}.rs      │
│    crates/ryframe-db/src/repos/{table}_repo.rs    │
│    crates/ryframe-service/src/{table}_service.rs  │
│    crates/ryframe-api/src/handlers/{table}_handler.rs │
│    crates/ryframe-api/src/dto/{table}_dto.rs      │
└──────────────────────────────────────────────────┘
```

#### 6.12.3 数据库类型映射

| PostgreSQL | MySQL | Rust 类型 |
|------------|-------|-----------|
| `integer` / `int4` | `int` | `i32` |
| `bigint` / `int8` | `bigint` | `i64` |
| `varchar` / `text` | `varchar` / `text` | `String` |
| `boolean` | `tinyint(1)` | `bool` |
| `timestamp` | `datetime` | `chrono::NaiveDateTime` |
| `jsonb` / `json` | `json` | `serde_json::Value` |
| `uuid` | `char(36)` | `uuid::Uuid` |
| `numeric` | `decimal` | `rust_decimal::Decimal` |

---

### 6.13 系统监控模块 (`ryframe-monitor`)

#### 6.13.1 监控能力矩阵

| 监控项 | 采集方式 | 数据来源 | 说明 |
|--------|----------|----------|------|
| **CPU 使用率** | 系统调用 | sysinfo crate | 核心数、使用率百分比 |
| **内存信息** | sysinfo crate | 系统 API | 总内存、已用、可用 |
| **磁盘信息** | sysinfo crate | 挂载点扫描 | 各分区总量、已用 |
| **进程信息** | std::process | 自身进程 | PID、启动时间、运行时长 |
| **健康检查** | 主动探测 | 内部组件 | DB 连通性、Redis 连通性 |
| **在线用户** | JWT 令牌池 | Redis / 内存 | 当前活跃令牌数 |
| **缓存统计** | 中间件拦截 | Redis | 命中率、键数量 |

#### 6.13.2 健康检查端点

```
GET /api/v1/monitor/health

Response:
{
  "status": "UP",
  "components": {
    "db": { "status": "UP", "latency_ms": 2 },
    "redis": { "status": "UP", "latency_ms": 1 },
    "disk_space": { "status": "UP", "free_gb": 45 }
  }
}
```

---

## 七、请求完整生命周期

以"用户登录"为例，展示一次请求经过的完整链路：

```
客户端 POST /api/v1/auth/login { "username": "admin", "password": "123456" }

  │
  ▼
┌──────────────────────────────────────────────────────────────┐
│ 1. RequestIdMiddleware                                       │
│    为请求生成 trace_id: "0193a8b2-7c1e-..."                   │
└──────────────────────────┬───────────────────────────────────┘
                           ▼
┌──────────────────────────────────────────────────────────────┐
│ 2. CorsMiddleware                                            │
│    校验 Origin，设置响应头                                     │
└──────────────────────────┬───────────────────────────────────┘
                           ▼
┌──────────────────────────────────────────────────────────────┐
│ 3. RequestLogMiddleware                                      │
│    记录 "POST /api/v1/auth/login" 开始处理                    │
└──────────────────────────┬───────────────────────────────────┘
                           ▼
┌──────────────────────────────────────────────────────────────┐
│ 4. AuthRouter (公开路由，跳过认证中间件)                        │
│    AuthHandler::login(State(app_state), Json(login_dto))     │
│    ├── validator 校验 username/password 非空                  │
│    ├── 调用 AuthService::login(username, password)            │
│    │   ├── UserRepository::find_by_username(username)         │
│    │   │   └── SeaORM: User::find().filter(...).one(db)       │
│    │   ├── argon2::verify(password, user.password_hash)       │
│    │   ├── RoleRepository::find_user_roles(user_id)           │
│    │   ├── jwt::encode(claims) → access_token, refresh_token  │
│    │   └── 返回 LoginResult { tokens, user_info }             │
│    └── 构造统一响应: AppResult<Json<LoginResponse>>           │
└──────────────────────────┬───────────────────────────────────┘
                           ▼
┌──────────────────────────────────────────────────────────────┐
│ 5. Response                                                  │
│    HTTP 200 { code: 200, data: { access_token: "...", ... }}  │
│    RequestLogMiddleware 记录 "[200] POST /api/v1/auth/login   │
│                             耗时: 23ms"                        │
└──────────────────────────────────────────────────────────────┘
```

---

## 八、错误处理体系

### 8.1 统一错误类型 (`ryframe-common::error`)

```
AppError 枚举:
├── Validation(String)              # 参数校验失败
├── Authentication(String)          # 认证失败 (401)
├── Authorization(String)           # 授权失败 (403)
├── NotFound(String)                # 资源不存在 (404)
├── Conflict(String)                # 数据冲突 (409)
├── Database(String)                # 数据库错误
├── Config(String)                  # 配置错误
└── Internal(String)                # 内部未知错误 (500)
```

### 8.2 错误转换与响应

通过为 `AppError` 实现 `IntoResponse` trait，自动将错误转换为统一 JSON 响应：

```
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            AppError::Validation(msg) => (StatusCode::BAD_REQUEST, 400, msg),
            AppError::Authentication(msg) => (StatusCode::UNAUTHORIZED, 401, msg),
            AppError::Authorization(msg) => (StatusCode::FORBIDDEN, 403, msg),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, 404, msg),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, 409, msg),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, 500, "服务器内部错误"),
        };
        // 构造 JSON 响应体
        ...
    }
}
```

---

## 九、测试策略

### 9.1 单元测试

- 每个 crate 的 `src` 目录下内联单元测试（`#[cfg(test)]` 模块）
- 使用 `mockall` Mock Repository trait，隔离数据库依赖
- Service 层测试不需要真实数据库

### 9.2 集成测试

- 位于项目根 `tests/` 目录
- 使用真实测试数据库（SQLite 内存模式或 Docker 临时 PostgreSQL）
- 覆盖完整 API 请求链路：HTTP Request → Router → Handler → Service → Repository → DB → Response

### 9.3 API 测试辅助工具

```
tests/common/mod.rs:
  - spawn_test_app() → (Router, TestDb)
  - create_test_user(app, role) → User
  - login_and_get_token(app) → String
  - assert_json(response, expected_json)
```

---

## 十、扩展指南

### 10.1 如何添加新的业务模块

1. 在 `ryframe-db/src/entities/` 添加实体定义
2. 在 `ryframe-db/src/repositories/` 添加 Repository 实现
3. 在 `ryframe-core/src/service.rs` 定义 Service trait
4. 在 `ryframe-service/src/` 实现 Service
5. 在 `ryframe-api/src/dto/` 添加请求/响应 DTO
6. 在 `ryframe-api/src/handlers/` 添加 Handler
7. 在 `ryframe-api/src/router.rs` 注册路由
8. 在 `ryframe/src/app.rs` 注入依赖

### 10.2 如何添加新的数据源

1. 在 `ryframe-config/src/db_config.rs` 添加新数据源配置
2. 在 `ryframe-db/src/connection.rs` 添加连接创建逻辑
3. 在 Repository 层通过 `DataSourceKey` 选择数据源

### 10.3 如何切换 Web 框架

1. 抽象 `ryframe-core` 中的 Handler trait 保持不变
2. 修改 `ryframe-api` 中的框架特定代码（Axum → Actix-Web）
3. 其余 crate 不感知 Web 框架变更

---

## 十一、部署架构

### 11.1 推荐部署拓扑

```
                    ┌─────────────┐
                    │   Nginx     │  ← 反向代理 + SSL 终结 + 静态资源
                    └──────┬──────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
        ┌──────────┐ ┌──────────┐ ┌──────────┐
        │ RyFrame  │ │ RyFrame  │ │ RyFrame  │  ← 应用实例 (多副本)
        │ Instance1│ │ Instance2│ │ Instance3│
        └────┬─────┘ └────┬─────┘ └────┬─────┘
             │            │            │
             └────────────┼────────────┘
                          │
              ┌───────────┼───────────┐
              ▼           ▼           ▼
        ┌──────────┐ ┌──────────┐ ┌──────────┐
        │PostgreSQL│ │  Redis   │ │   MinIO  │  ← 数据层
        │ (主+从)  │ │ (缓存)   │ │ (文件存储)│
        └──────────┘ └──────────┘ └──────────┘
```

### 11.2 Docker 构建

```
# 多阶段构建
1. Builder 阶段: rust:1.85-slim + cargo build --release
2. Runtime 阶段: debian:bookworm-slim (仅复制二进制)
3. 最终镜像大小: ~20-30 MB

# Docker Compose
version: '3.8'
services:
  app:
    build: .
    ports: ["8080:8080"]
    environment:
      - APP_ENV=prod
    depends_on:
      - postgres
      - redis
  postgres:
    image: postgres:16-alpine
  redis:
    image: redis:7-alpine
```

---

## 十二、版本规划

| 版本 | 里程碑 | 核心交付 | 状态 |
|------|--------|----------|------|
| **v0.1.0** | 基础骨架 | Workspace 搭建、配置管理、数据库连接池、基础 CRUD、统一错误处理 | ✅ 已完成 |
| **v0.2.0** | 认证授权 | JWT 登录/刷新、RBAC 权限模型、认证/授权中间件、验证码 | ✅ 已完成 |
| **v0.3.0** | 系统管理 | 用户/角色/菜单/部门/岗位管理、字典管理、参数配置、通知公告 | ✅ 已完成 |
| **v0.4.0** | 监控运维 | 操作日志/登录日志、在线用户、服务监控、健康检查端点 | ✅ 已完成 |
| **v0.5.0** | 高级特性 | 定时任务调度、代码生成器、数据权限、XSS 防护、缓存策略、消息队列、多租户、熔断器 | ✅ 已完成 |
| **v0.6.0** | 生产就绪 | Docker 容器化部署、K8s 清单、Grafana/Prometheus 监控、冒烟/压力测试 | 🔄 进行中 |
| **v1.0.0** | 正式发布 | 完整 API 文档、测试覆盖率 70%+、cargo audit 零漏洞、CI/CD 流水线 | 📅 规划中 |

---

## 十三、与原架构对照改进总结

| 改进维度 | 原设计 | 优化后设计（当前实际状态） |
|----------|--------|------------|
| **Crate 数量** | 9 个 | 13 个（12 功能 crate + 1 宏 crate） |
| **模块分层** | 平面树形依赖 | 四层分层模型（基础共享→基础设施→领域→接入→入口） |
| **实体设计** | 5 张表 | 19 张表（15 核心 + job/job_log/role_dept + 用户生成） |
| **API 路由** | 14 个端点 | 80+ 个端点，分组为 auth/system/monitor/tools/common |
| **中间件** | 5 个 | 15 个（Metrics/Telemetry/RequestId/Compression/CORS/RequestLog/XssFilter/Timeout/BodyLimit/ApiRateLimit/RateLimit/SecurityHeaders/CacheControl/Idempotency/ReplayProtection） |
| **高级特性** | 无 | 缓存策略（防穿透/击穿/雪崩）、消息队列（Kafka）、多租户、熔断器、分布式锁、事件总线、功能开关、gRPC |
| **可观测性** | 基础日志 | OpenTelemetry 链路追踪 + Prometheus Metrics + 结构化日志 |
| **部署** | 无 | Docker 多阶段构建 + docker-compose + K8s all-in-one + Grafana/Prometheus + 冒烟/压力测试 |
| **开发者体验** | 基础 | OpenAPI + Swagger UI + 国际化 + 代码生成器 + 配置热加载 |
| **安全** | JWT | JWT + 多层限流 + XSS 过滤 + 安全头 + 幂等性 + 重放防护 + Token 黑名单 |

---

> **文档版本**：v3.0  
> **最后更新**：2026-05-31  
> **变更说明**：全面更新至项目当前状态——补充 5.1 节实际文件结构（所有 crate 均已实现）、中间件从 7 个扩展至 15 个、新增缓存/消息队列/多租户/弹性组件章节（6.7-6.10）、更新版本规划状态、修正部署配置（端口 8080、MySQL 8.0）、增强对照表  
> **维护者**：RyFrame 开发团队
