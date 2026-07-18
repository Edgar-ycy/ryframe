# RyFrame

RyFrame 是一个基于 Rust 2024 的后台管理系统框架，采用 Cargo Workspace 组织后端模块，并配套独立维护的 Vue 3 管理端。项目目标是提供一套可直接启动、便于扩展的企业级后台基础能力。

## 特性

- 认证授权：内存 access token、HttpOnly refresh Cookie、CSRF 防护、会话轮换、RBAC 权限和数据权限。
- 系统管理：用户、角色、权限、菜单、部门、岗位、参数、字典、通知、日志。
- 安全中间件：限流、XSS 过滤、请求日志、CORS、超时、请求体限制、安全响应头、幂等与重放防护。
- 数据与缓存：MySQL 8.4、SeaORM 主库/多只读副本、命名业务数据源、Rust Migrator、Redis 分布式状态。
- 监控运维：存活与就绪探针、服务状态、缓存统计、数据库连接池、Prometheus 指标。
- 扩展能力：代码生成、RustFS/MinIO/S3 对象存储、文件上传下载、Excel 导入导出、国际化和 WebSocket。
- 前端管理端：独立仓库 [ryframe-vue3](https://github.com/Edgar-ycy/ryframe-vue3) 提供 Vue 3 + TypeScript + Element Plus 后台界面。

## 快速开始

### 环境要求

- Rust 1.85+
- MySQL 8.4
- Redis 7+；生产环境强制使用并要求持久化与 `noeviction`，开发环境才允许显式的内存降级
- RustFS；开发配置默认连接本机 `9000` 端口，也可显式切换为本地存储

### 后端

```bash
git clone https://github.com/Edgar-ycy/ryframe.git
cd ryframe

# MySQL 示例：创建空数据库，启动时由 Rust Migrator 初始化
mysql -u root -p -e "CREATE DATABASE IF NOT EXISTS ryframe_config DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_general_ci;"
mysql -u root -p -e "CREATE DATABASE IF NOT EXISTS ryframe_device DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_general_ci;"

# 仅本机开发使用以下 Redis 端口绑定；生产必须另外配置 TLS、网络隔离和 ACL
docker run -d --name ryframe-redis -p 127.0.0.1:6379:6379 -v ryframe-redis-data:/data -v "$PWD/deploy/redis/redis.conf:/usr/local/etc/redis/redis.conf:ro" redis:7-alpine redis-server /usr/local/etc/redis/redis.conf --bind 0.0.0.0 --protected-mode no

# 启动与 CI 相同版本的 RustFS；已有本机实例时无需重复执行
docker run -d --name ryframe-rustfs -p 9000:9000 -p 9001:9001 -e RUSTFS_ACCESS_KEY=rustfsadmin1 -e RUSTFS_SECRET_KEY=rustfsadmin1 -v ryframe-rustfs-data:/data rustfs/rustfs:1.0.0-beta.8

# 按本地环境修改数据库、Redis、对象存储等配置
# config/app.dev.toml

cargo run
```

默认服务地址：

- API：`http://localhost:8080`
- 存活探针：`http://localhost:8080/livez`
- 就绪探针：`http://localhost:8080/readyz`
- Swagger UI：`http://localhost:8080/api/v1/swagger-ui`
- Prometheus：`http://localhost:8080/api/v1/monitor/metrics`

默认账号：

| 账号 | 密码 | 说明 |
| --- | --- | --- |
| `admin` | `123456` | 超级管理员 |
| `user` | `123456` | 普通用户 |

### 前端

前端是独立 Git 仓库，本地开发时固定检出到后端工作区的 `ryframe-vue3/` 目录；所有 `pnpm` 命令必须从该目录执行：

```bash
git clone https://github.com/Edgar-ycy/ryframe-vue3.git ryframe-vue3
cd ryframe-vue3
pnpm install
pnpm dev
```

生产构建：

```bash
cd ryframe-vue3
pnpm build
```

## 常用命令

```bash
cargo check --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python scripts/check_source_hygiene.py
python scripts/check_architecture.py
cargo run -p ryframe-api --bin export_openapi -- openapi/openapi.json
# 构建 Linux 可执行文件
cross build --release --target x86_64-unknown-linux-gnu
```

## 重置数据库

```powershell
$env:APP_ENV = "dev"
cargo run -p ryframe --bin ryframe-db-reset -- `
  --database ryframe_config `
  --confirm-reset RESET-RYFRAME-DATABASE
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
│   ├── ryframe-db/           # SeaORM 实体、仓储、事务和数据库拓扑
│   ├── ryframe-db-migration/ # 数据库迁移
│   ├── ryframe-auth/         # 认证、授权、权限中间件
│   ├── ryframe-core/         # 分页、缓存、租户上下文、分布式锁与熔断
│   ├── ryframe-config/       # 配置加载与环境覆盖
│   ├── ryframe-common/       # 公共类型、错误、工具函数、国际化
│   ├── ryframe-middleware/   # 通用中间件
│   ├── ryframe-monitor/      # 监控与健康检查
│   ├── ryframe-generator/    # 代码生成
│   ├── ryframe-storage/      # 本地与 RustFS/MinIO/S3 对象存储端口及实现
│   └── ryframe-macro/        # 过程宏
├── config/                   # app.toml 与环境配置
├── docs/                     # 使用指南与架构文档
├── openapi/openapi.json      # CI 校验并发布的规范 API 快照
├── scripts/                  # 源码、权限和架构门禁
├── locales/                  # 国际化资源
├── sql/                      # Migrator 生成并由 CI 校验的只读 MySQL 快照
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
| `APP_DATABASE_REPLICAS` | 命名只读副本 JSON 数组 | `[]` |
| `APP_DATABASE_SOURCES` | 命名业务数据源 JSON 数组 | 按环境配置 |
| `APP_GENERATOR_DATA_SOURCE` | 代码生成器读取的数据源名 | `primary` |
| `APP_REDIS_MODE` | `required`、`optional` 或 `disabled`；生产固定 required | `optional` |
| `APP_PROXY_TRUSTED_CIDRS` | 可以提供转发头的 Nginx CIDR 数组 | `[]` |
| `APP_OBJECT_STORAGE_BACKEND` | `local`、`rustfs`、`minio` 或 `s3` | 按环境配置 |
| `APP_OBJECT_STORAGE_ENDPOINT` | RustFS/MinIO/S3 API 地址 | 按环境配置 |
| `OTEL_ENABLED` | 是否启用链路追踪 | `false` |
| `OTEL_ENDPOINT` | OTLP 上报地址 | `http://localhost:4318/v1/traces` |
| `OTEL_SERVICE_NAME` | 服务名 | `ryframe` |
| `OTEL_SAMPLE_RATE` | 采样率 | `1.0` |

生产环境不要把密钥、数据库密码、对象存储凭据写入仓库，建议通过环境变量或部署平台注入。
配置在启动时完成解密和严格校验；配置文件或环境变量变化后必须重启进程才会生效。

## 文档

- [文档索引](docs/README.md)
- [API 使用指南](docs/api-guide.md)
- [架构说明](docs/architecture.md)
- [数据库指南](docs/db-guide.md)
- [对象存储与 RustFS 指南](docs/storage-guide.md)
- [前端集成指南](docs/frontend-integration.md)


## 许可

[MIT](LICENSE)
