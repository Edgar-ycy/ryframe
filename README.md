# RyFrame

RyFrame 是面向企业后台的 Rust 2024 服务端，与 RyFrame-Vue3 配套使用。它提供认证授权、系统管理、多租户、异步任务、筛选导出、对象存储和可观测性能力。

## 环境准备

本地开发使用 Windows，需要准备：

- Rust 1.97；
- MySQL；
- WSL 中的 Redis；
- 需要文件能力时启动 Windows RustFS；
- 前端所需的 Node.js 与 pnpm。

配置从 `config/` 中对应环境的文件加载，并可使用 `APP_` 环境变量覆盖。密码、令牌和证书请使用本机环境变量或密钥文件，不要写入配置样例。

## 启动开发环境

```powershell
$env:APP_ENV = "dev"
cargo migrate verify
cargo dev
```

`cargo migrate verify` 校验控制库结构，不修改数据库。需要更新本地数据库时运行 `cargo migrate up`；租户数据目标可使用 `cargo migrate verify tenant-data --all` 校验。

`cargo dev` 同时管理 API、Worker 和 Vite，并在后端修改后完成探活再切换版本。按 `Ctrl+C` 可停止整组进程。

只在排障时单独启动 Worker，并让 API 使用 external 任务模式：

```powershell
$env:APP_ENV = "dev"
$env:APP_JOBS_MODE = "external"
cargo run --locked -p ryframe --bin ryframe-worker
```

## 开发与检查

日常修改后运行：

```powershell
cargo verify
```

准备联调或交付前运行完整检查：

```powershell
cargo verify --full
```

`--scope backend|frontend` 可限制主要检查侧。更多迁移、测试和排障命令见[开发指南](docs/development.md)。

## 同步 API 契约

`openapi/openapi.json` 是后端 HTTP 契约快照。接口变化后同步前端派生契约：

```powershell
cargo api-sync
```

完成同步后，在前端运行消费者检查并进行浏览器联调。请求格式、认证方式和稳定错误码见 [API 指南](docs/api.md)。

## 开发标准资源

标准 CRUD 资源通过资源清单离线生成。以下命令分别用于预览、写入和查看生成链路：

```powershell
cargo resource post
cargo resource post --write
cargo resource post --explain
```

资源清单位于 `catalog/resources/`。Post 和 Notice 可作为标准资源示例；导出、消息发布等特殊行为使用普通 Rust 用例扩展。完整流程见[开发指南](docs/development.md)。

## 文档

- [架构与扩展位置](docs/architecture.md)
- [开发指南](docs/development.md)
- [API 使用](docs/api.md)
- [数据与迁移](docs/data.md)
- [部署与排障](docs/operations.md)

字段、菜单、权限、配置默认值和生成结果分别以 OpenAPI、`catalog/access.toml`、配置结构、资源清单及命令 `--help` 为准。
