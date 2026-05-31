# RyFrame

[![Rust](https://img.shields.io/badge/rust-1.95%2B-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/Edgar-ycy/ryframe/actions/workflows/ci.yml/badge.svg)](https://github.com/Edgar-ycy/ryframe/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/Edgar-ycy/ryframe/branch/main/graph/badge.svg)](https://codecov.io/gh/Edgar-ycy/ryframe)

**RyFrame** —— 基于 Rust + Axum 的现代化企业级后端框架。

## 特性

- **认证授权**：JWT 登录/刷新 + RBAC 权限模型
- **系统管理**：用户/角色/菜单/部门/岗位/参数/字典/通知 完整 CRUD
- **安全防护**：XSS 过滤、限流中间件、数据权限、操作日志
- **Redis 缓存**：配置/字典/菜单树/部门树 读缓存 + 写失效
- **监控运维**：服务器信息、健康检查、在线用户、Prometheus Metrics
- **定时任务**：Cron 调度 + 任务管理 + 执行历史
- **代码生成**：读取表结构自动生成 CRUD 代码
- **弹性容错**：重试（指数退避）+ 熔断器
- **Swagger UI**：交互式 API 文档
- **Docker**：多阶段构建 + docker-compose 一键部署

## 快速开始

### 环境要求

- Rust 1.95+
- MySQL 8.0 或 PostgreSQL 15+（可选 Redis 7+）

### 本地开发

```bash
# 克隆项目
git clone https://github.com/your-org/ryframe.git
cd ryframe

# 初始化数据库
# 执行 sql/ryframe_config.sql 创建数据库和表

# 配置连接
# 编辑 config/app.dev.toml 中的数据库连接信息

# 启动服务
cargo run

# 访问
# API: http://localhost:3000
# Health: http://localhost:3000/health
# Metrics: http://localhost:3000/metrics
# Swagger UI: http://localhost:3000/api/v1/swagger-ui
```

### Docker 部署

```bash
docker compose up -d
```

### Kubernetes 部署

```bash
# 使用 Helm Chart
helm install ryframe ./deploy/helm/ryframe

# 生产环境覆盖
helm install ryframe ./deploy/helm/ryframe -f ./deploy/helm/ryframe/values-prod.yaml
```

### 默认账户

| 账号 | 密码 |
|------|------|
| admin | admin123 |

## 项目结构

```
ryframe/
├── config/              # 配置文件（dev/test/prod）
├── sql/                 # 数据库初始化脚本
├── deploy/              # Nginx 配置
├── crates/
│   ├── ryframe/         # 应用入口（main.rs）
│   ├── ryframe-api/     # HTTP 层（Handler/DTO/Router）
│   ├── ryframe-service/ # 业务逻辑层
│   ├── ryframe-db/      # 数据访问层（Entity/Repository）
│   ├── ryframe-core/    # 核心抽象（Context/Redis/Resilience）
│   ├── ryframe-auth/    # 认证授权（JWT/RBAC/Password）
│   ├── ryframe-config/  # 配置管理
│   ├── ryframe-common/  # 公共工具（错误/常量/工具函数）
│   ├── ryframe-middleware/ # 中间件（限流/XSS/Metrics/日志）
│   ├── ryframe-monitor/ # 服务监控
│   ├── ryframe-task/    # 定时任务
│   └── ryframe-generator/ # 代码生成器
└── docs/                # 架构文档 / 开发计划
```

## API 端点概览

| 模块 | 前缀 | 说明 |
|------|------|------|
| 认证 | `/api/v1/auth` | 登录/登出/刷新/个人中心 |
| 系统管理 | `/api/v1/system` | 用户/角色/菜单/部门/岗位/字典/配置 |
| 监控 | `/api/v1/monitor` | 服务器/健康检查/Prometheus/缓存 |
| 定时任务 | `/api/v1/system/jobs` | 任务 CRUD/暂停/恢复/触发 |
| 代码生成 | `/api/v1/tools/gen` | 表结构读取 → 代码生成 |
| 通用 | `/api/v1/common` | 文件上传/下载 |
| API 文档 | `/api/v1/swagger-ui` | Swagger UI 交互文档 |
| API 规范 | `/api/v1/api-docs/openapi.json` | OpenAPI 3.0 JSON |

## 运行测试

```bash
# 全量测试（包括集成测试）
cargo test --workspace

# 仅单元测试
cargo test --workspace --lib

# 性能基准
cargo bench --workspace

# 代码检查
cargo clippy --workspace
```

## 技术栈

| 层次 | 技术 |
|------|------|
| Web 框架 | Axum 0.8 |
| 异步运行时 | Tokio |
| ORM | SeaORM 2.0 |
| 认证 | JWT (jsonwebtoken) + Argon2 |
| 缓存 | Redis (redis-rs) |
| 日志 | Tracing + Tracing-subscriber |
| 序列化 | Serde + TOML |
| 数据库 | MySQL / PostgreSQL / SQLite |
| 监控 | Prometheus |
| API 文档 | Utoipa + Swagger UI |

## 许可证

MIT License
