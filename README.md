# RyFrame

RyFrame 是一个基于 Rust 2024 的后台管理系统框架，采用 Cargo Workspace 组织后端模块，并配套独立维护的 Vue 3 管理端。项目目标是提供一套可直接启动、便于扩展的企业级后台基础能力。

## 特性

- 认证授权：内存 access token、HttpOnly refresh Cookie、CSRF 防护、会话轮换、RBAC 权限和数据权限。
- 系统管理：用户、角色、权限、菜单、部门、岗位、参数、字典、通知、日志。
- 安全中间件：限流、请求日志、CORS、超时、请求体限制、安全响应头、幂等与重放防护。
- 数据与缓存：MySQL 8.4、SeaORM 主库/多只读副本、命名业务数据源、Rust Migrator、Redis 分布式状态。
- 监控运维：存活探针、后台依赖探测与缓存式就绪快照、服务状态、缓存统计、数据库连接池、Prometheus 指标。
- 扩展能力：代码生成、RustFS/MinIO/S3 对象存储、文件上传下载、Excel 导入导出、国际化和 WebSocket。
- 前端管理端：独立仓库 [ryframe-vue3](https://github.com/Edgar-ycy/ryframe-vue3) 提供 Vue 3 + TypeScript + Element Plus 后台界面。

## 快速开始

### 环境要求

- Rust 1.97.1（仓库通过 `rust-toolchain.toml` 固定）
- Python 3.11+（用于后端质量门禁与发布校验脚本）
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

# 启动仓库本地集成环境固定版本的 RustFS；已有本机实例时无需重复执行
docker run -d --name ryframe-rustfs -p 9000:9000 -p 9001:9001 -e RUSTFS_ACCESS_KEY=rustfsadmin1 -e RUSTFS_SECRET_KEY=rustfsadmin1 -v ryframe-rustfs-data:/data rustfs/rustfs:1.0.0-beta.8

# 按本地环境修改数据库、Redis、对象存储等配置
# config/app.dev.toml

cargo run
```

首次启动前可运行 `cargo xtask doctor` 检查 Rust、Node、pnpm、前后端仓库和配置。联合生产检查使用 `cargo xtask check`，可附加 `--scope backend` 或 `--scope frontend` 缩小范围。Cargo feature 必须在 `config/feature-matrix.json` 登记最小与最大组合，并在本地运行 `cargo xtask feature-matrix`；CI 只校验注册表元数据，不重复编译矩阵。稳定发布前使用 `cargo xtask release-verify` 校验双仓库版本、提交和发布元数据。`file-maintenance` 只用于一次性历史文件校验，常规 API 和 Worker 不会启用它。

`xtask` 会在后端 `target/corepack-bin/` 中创建临时 Corepack shim，并从前端的 `packageManager` 字段读取固定的 pnpm 版本；该目录是本机构建缓存，不应提交。

默认服务地址：

- API：`http://localhost:8080`
- 存活探针：`http://localhost:8080/livez`
- 就绪探针：`http://localhost:8080/readyz`
- Swagger UI：`http://localhost:8080/api/v1/swagger-ui`
- Prometheus：`http://localhost:8080/api/v1/monitor/metrics`

`cargo run` 默认启用 `runtime-swagger-ui`，因此本机开发可以访问上述 Swagger UI。生产镜像和
手工生产构建必须使用 `--no-default-features`，以便不把内嵌 Swagger UI 静态资源带入 API
二进制；此时 `APP_API_DOCS_ENABLED` 必须保持为 `false`。

`/readyz` 只读取后台任务最近一次依赖探测的内存快照，不在请求路径执行 SQL、Redis
或对象存储网络调用；快照过期或必要依赖不可用时返回 `503`。

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
corepack enable
pnpm install
pnpm dev
```

生产构建：

```bash
cd ryframe-vue3
pnpm build
```

## 常用命令

以下命令用于本地开发与生产构建验证。测试、基准和验收资产只保留在维护者本机的忽略目录，
不纳入 Git、提交或 CI：

```powershell
cargo check --workspace --lib --bins
cargo fmt --all -- --check
cargo clippy --workspace --lib --bins -- -D warnings
python scripts/check_source_hygiene.py
python scripts/check_architecture.py
cargo run --locked -p ryframe-api --bin export_openapi -- openapi/openapi.json
cargo run --locked -p ryframe-db-migration --bin export_mysql_snapshot -- sql/ryframe_config.sql
# 部署环境按需从稳定标签源码构建不含 Swagger UI 的 Linux API 可执行文件
cross build --release --no-default-features --target x86_64-unknown-linux-gnu -p ryframe --bin ryframe
```

托管后端 CI 在 Linux 上执行格式、源码卫生、架构与权限门禁、工作流和部署静态校验，
并只通过一次生产库和可执行文件的 Clippy 编译；依赖安全审计在独立作业执行。OpenAPI 与 MySQL
快照由开发者在本地生成并检入，托管 CI 不再为快照重复编译；稳定版 Release 也不重复
编译或测试，只接受两仓同名 annotated tag 解引用出的精确提交已成功完成各自 push CI 的
证据；GitHub Release 不生成额外交付身份文件，也不上传自定义附件。

## 重置数据库

```powershell
$env:APP_ENV = "dev"
cargo run -p ryframe --bin ryframe-db-reset -- `
  --database ryframe_config `
  --confirm-reset RESET-RYFRAME-DATABASE
```

## 目录结构

```text
.
├── crates/
│   ├── ryframe/              # 应用、迁移与独立 Worker 的可执行入口
│   ├── ryframe-api/          # HTTP 路由、处理器、DTO、OpenAPI 与消息 WebSocket
│   ├── ryframe-service/      # 业务用例、后台任务与消息中心
│   ├── ryframe-db/           # SeaORM 实体、仓储、事务和数据库拓扑
│   ├── ryframe-db-migration/ # 数据库迁移
│   ├── ryframe-auth/         # 认证、授权、权限中间件
│   ├── ryframe-kernel/       # 传输无关的领域类型、错误码与主体上下文
│   ├── ryframe-http/         # HTTP 错误映射与统一响应信封
│   ├── ryframe-i18n/         # 显式注入的语言协商、资源校验与文本渲染
│   ├── ryframe-utils/        # 雪花 ID、脱敏、差异与文件处理等通用工具
│   ├── ryframe-captcha/      # 验证码生成与图像渲染
│   ├── ryframe-excel/        # Excel 导入导出
│   ├── ryframe-core/         # 分页、缓存、租户上下文、分布式锁与熔断
│   ├── ryframe-config/       # 配置加载与环境覆盖
│   ├── ryframe-middleware/   # 通用中间件
│   ├── ryframe-monitor/      # 监控与健康检查
│   ├── ryframe-generator/    # 代码生成
│   ├── ryframe-storage/      # 本地与 RustFS/MinIO/S3 对象存储端口及实现
│   └── ryframe-macro/        # 过程宏
├── config/                   # app.toml 与环境配置
├── docs/                     # 使用指南与架构文档
├── openapi/openapi.json      # 本地生成、联合发布比对的规范 API 快照
├── scripts/                  # 源码、权限和架构门禁
├── locales/                  # 国际化资源
├── sql/                      # Migrator 在本地生成的只读 MySQL 快照
└── deploy/                   # 部署相关资源
```

## 配置

配置文件按默认值到环境覆盖的顺序加载：

```text
config/app.toml
config/app.dev.toml
config/app.prod.toml
```

常用环境变量：

| 变量 | 说明 | 默认值 |
| --- | --- | --- |
| `APP_ENV` | 运行环境：`dev`、`prod` | `dev` |
| `SNOWFLAKE_WORKER_ID` | Snowflake 节点 ID（`0..=1023`）；生产环境必填，且每个同时运行的实例必须唯一 | 开发环境默认 `1` |
| `APP_CONFIG_DIR` | 配置目录 | `config` |
| `APP_DATABASE_REPLICAS_FILE` | 命名只读副本 JSON 数组文件 | `[]` |
| `APP_DATABASE_SOURCES_FILE` | 命名业务数据源 JSON 数组文件 | 按环境配置 |
| `APP_DATABASE_SQL_LOG_LEVEL` | SQL 日志模式：`off`、`slow`、`summary`、`full`；生产常态必须为 `off` | 开发 `slow`，生产 `off` |
| `APP_DATABASE_SQL_SLOW_THRESHOLD_MS` | 慢 SQL 阈值（毫秒，`1..=60000`） | `200` |
| `APP_GENERATOR_DATA_SOURCE` | 代码生成器读取的数据源名 | `primary` |
| `APP_AUTH_JWT_SECRET_FILE` | JWT 签名密钥文件；生产环境必须随机生成且至少 32 字节 | 开发配置值 |
| `APP_REDIS_MODE` | `required`、`optional` 或 `disabled`；生产固定 required | `optional` |
| `APP_REDIS_TLS` | Redis 是否使用证书校验的 `rediss://`；远程生产 Redis 必须启用 | `false` |
| `APP_JOBS_MODE` | 后台任务执行模式；生产固定 `external` | 开发 `embedded`，生产 `external` |
| `APP_JOBS_POLL_INTERVAL_MS` | 空队列轮询的最小等待（`50..=60000`）；领取任务、收到唤醒或手动重试后会重置到该值 | `500` |
| `APP_JOBS_MAX_IDLE_POLL_INTERVAL_MS` | 连续空闲时按 2 倍退避的上限，必须位于最小等待到 `60000` 毫秒之间 | `5000` |
| `APP_JOBS_LEASE_RECOVERY_INTERVAL_SECONDS` | 过期任务和 Outbox 租约恢复的独立周期（`1..=3600`） | `15` |
| `APP_DATABASE_TLS_MODE` | MySQL TLS 策略；远程生产数据库使用 `verify_identity` | `disabled` |
| `APP_PROXY_TRUSTED_CIDRS` | 可以提供转发头的 Nginx CIDR 数组 | `[]` |
| `APP_API_DOCS_ENABLED` | 是否暴露运行时 Swagger/OpenAPI；生产必须关闭 | `true` |
| `APP_MONITOR_METRICS_BEARER_TOKEN_FILE` | Prometheus 专用 Bearer Token 文件；生产至少 32 字节 | 空 |
| `APP_OBJECT_STORAGE_BACKEND` | `local`、`rustfs`、`minio` 或 `s3` | 按环境配置 |
| `APP_OBJECT_STORAGE_ENDPOINT` | RustFS/MinIO/S3 API 地址 | 按环境配置 |
| `APP_OBJECT_STORAGE_USE_SSL` | 远程对象存储是否使用 TLS；生产还要求 `https://` endpoint | `false` |
| `APP_OBJECT_STORAGE_ALLOW_LOCAL_IN_PRODUCTION` | 显式确认生产单实例/共享卷使用本地存储 | `false` |
| `APP_TELEMETRY_ENABLED` | 是否启用链路追踪 | `false` |
| `APP_TELEMETRY_ENDPOINT` | OTLP 上报地址 | `http://localhost:4318/v1/traces` |
| `APP_TELEMETRY_SERVICE_NAME` | 服务名 | `ryframe` |
| `APP_TELEMETRY_SAMPLE_RATIO` | 根 Span 采样率 | `0.1` |
| `APP_TELEMETRY_EXPORT_TIMEOUT_SECS` | 单次 OTLP 导出最大等待秒数 | `5` |
| `APP_TELEMETRY_MAX_QUEUE_SIZE` | 批量导出前允许暂存的最大 Span 数 | `2048` |

生产 Compose 通过 `APP_*_FILE` 读取 Docker secret；文件必须是 UTF-8，末尾的一个换行会被移除。同一配置不能同时设置直接值和 `_FILE`。不要把密钥、数据库密码、对象存储凭据或副本连接 JSON 写入仓库。
配置在启动时完成合并和严格校验；当前不提供配置密文解密，任何使用旧 `ENC[...]` 格式的值都会被拒绝。配置文件或环境变量变化后必须重启进程才会生效。
容器镜像默认使用 `APP_ENV=prod`，启动时必须通过 `docker run -e SNOWFLAKE_WORKER_ID=<唯一节点号> ...` 或编排平台注入节点 ID；滚动发布期间新旧实例也不能复用同一个值。Snowflake 遇到时钟回拨或单毫秒序列耗尽会立即返回可重试的 503，不会等待或生成逻辑未来时间；由于时间戳高水位不跨重启持久化，同一 worker ID 只能在物理时间超过其最后生成时间后复用。
生产环境的数据库、Redis 和对象存储如果跨主机，必须启用证书校验的 TLS；多实例部署应使用 RustFS/MinIO/S3，本地存储只允许单实例或经过验证的共享持久卷。详细要求见[生产部署基线](docs/production-deployment.md)。

GitHub 稳定版 Release 只保留平台自动生成的源码 ZIP/TAR，不构建或分发可执行文件、前端
构建产物、容器镜像、SBOM、签名或其他自定义附件。部署环境必须从稳定标签源码独立构建、扫描并
固定自身产物摘要。

## 文档

- [文档索引](docs/README.md)
- [API 使用指南](docs/api-guide.md)
- [架构说明](docs/architecture.md)
- [数据库指南](docs/db-guide.md)
- [缓存命名空间一致性协议](docs/cache-namespace.md)
- [对象存储与 RustFS 指南](docs/storage-guide.md)
- [前端集成指南](docs/frontend-integration.md)
- [生产部署基线](docs/production-deployment.md)
- [生产监控与值班手册](docs/operations-runbook.md)
- [容量测试与验收标准](docs/capacity-guide.md)
- [稳定发布与回滚指南](docs/release-guide.md)


## 许可

[MIT](LICENSE)
