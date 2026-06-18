# RyFrame

RyFrame 是一个基于 Rust 2024 的后台管理系统框架，采用 Cargo Workspace 组织后端模块，并内置 Vue 3 管理端。项目目标是提供一套可直接启动、便于扩展的企业级后台基础能力。

## 特性

- 认证授权：JWT 登录、刷新、登出、RBAC 权限、数据权限。
- 系统管理：用户、角色、权限、菜单、部门、岗位、参数、字典、通知、日志。
- 安全中间件：限流、XSS 过滤、请求日志、CORS、超时、请求体限制、安全响应头、幂等与重放防护。
- 数据与缓存：SeaORM 数据访问、数据库初始化脚本、Redis 缓存、本地内存降级。
- 监控运维：健康检查、服务状态、缓存统计、数据库连接池、Prometheus 指标。
- 扩展能力：代码生成、对象存储、文件上传下载、Excel 导入导出、国际化、gRPC、WebSocket、消息队列。
- 前端管理端：`ryframe-vue3/` 提供 Vue 3 + TypeScript + Element Plus 后台界面。

## 快速开始

### 环境要求

- Rust 1.85+
- MySQL 8.0 或 PostgreSQL 15+
- Redis 7+，未配置时部分缓存能力会降级到内存模式
- Node.js 与 pnpm，用于运行前端

### 后端

```bash
git clone https://github.com/Edgar-ycy/ryframe.git
cd ryframe

# MySQL 示例：创建并初始化数据库
mysql -u root -p -e "CREATE DATABASE IF NOT EXISTS ryframe_config DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_general_ci;"
mysql -u root -p ryframe_config < sql/ryframe_config.sql

# 按本地环境修改数据库、Redis、对象存储等配置
# config/app.dev.toml

cargo run
```

默认服务地址：

- API：`http://localhost:8080`
- 健康检查：`http://localhost:8080/health`
- Swagger UI：`http://localhost:8080/api/v1/swagger-ui`
- Prometheus：`http://localhost:8080/api/v1/monitor/metrics`

默认账号：

| 账号 | 密码 | 说明 |
| --- | --- | --- |
| `admin` | `123456` | 超级管理员 |
| `user` | `123456` | 普通用户 |

### 前端

```bash
cd ryframe-vue3
pnpm install
pnpm dev
```

生产构建：

```bash
pnpm build
```

## 常用命令

```bash
cargo check --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# 构建 Linux 可执行文件
cross build --release --target x86_64-unknown-linux-gnu
```

如果安装了 `cargo-nextest`，推荐使用：

```bash
cargo nextest run --workspace
```

## 目录结构

```text
.
├── crates/
│   ├── ryframe/              # 应用入口
│   ├── ryframe-api/          # HTTP 路由、处理器、DTO、OpenAPI
│   ├── ryframe-service/      # 业务逻辑
│   ├── ryframe-db/           # 实体、仓储、迁移
│   ├── ryframe-auth/         # 认证、授权、权限中间件
│   ├── ryframe-core/         # 缓存、多数据源、事件、消息队列等核心能力
│   ├── ryframe-config/       # 配置加载与环境覆盖
│   ├── ryframe-common/       # 公共类型、错误、工具函数、国际化
│   ├── ryframe-middleware/   # 通用中间件
│   ├── ryframe-monitor/      # 监控与健康检查
│   ├── ryframe-generator/    # 代码生成
│   └── ryframe-macro/        # 过程宏
├── config/                   # app.toml 与环境配置
├── docs/                     # 使用指南与架构文档
├── locales/                  # 国际化资源
├── ryframe-vue3/             # Vue 3 管理端
├── sql/                      # 数据库初始化脚本
└── deploy/                   # 部署相关资源
```

## 配置

配置文件按默认值到环境覆盖的顺序加载：

```text
config/app.toml
config/app.dev.toml
config/app.test.toml
config/app.prod.toml
```

常用环境变量：

| 变量 | 说明 | 默认值 |
| --- | --- | --- |
| `APP_ENV` | 运行环境：`dev`、`test`、`prod` | `dev` |
| `APP_CONFIG_DIR` | 配置目录 | `config` |
| `OTEL_ENABLED` | 是否启用链路追踪 | `false` |
| `OTEL_ENDPOINT` | OTLP 上报地址 | `http://localhost:4318/v1/traces` |
| `OTEL_SERVICE_NAME` | 服务名 | `ryframe` |
| `OTEL_SAMPLE_RATE` | 采样率 | `1.0` |

生产环境不要把密钥、数据库密码、对象存储凭据写入仓库，建议通过环境变量或部署平台注入。

## 文档

- [文档索引](docs/README.md)
- [API 使用指南](docs/api-guide.md)
- [架构说明](docs/architecture.md)
- [数据库指南](docs/db-guide.md)
- [前端集成指南](docs/frontend-integration.md)

## 许可

[MIT](LICENSE)
