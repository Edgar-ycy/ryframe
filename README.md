# RyFrame

当前为 `0.x` 开发版本，安装和联调只支持当前 API、配置、任务载荷与数据库基线；请使用全新隔离数据库，不直接覆盖旧开发库。

当前提供用户、角色、权限、组织、产品套餐、多租户、配置迁移、跨库迁移、监控、消息、调度、导入和导出。Agent 查询接口、用户委托和服务账号管理已移除，个人资料、密码、头像和登录会话管理继续保留。套餐可以发布不含可选能力的版本，租户开通与配额管理照常使用。

RyFrame 是面向企业后台的 Rust 2024 服务端，与 RyFrame-Vue3 配套使用。它提供认证授权、系统管理、多租户、异步任务、筛选导出、对象存储和可观测性能力。

## 当前项目状态

API、Worker、迁移和维护程序按 feature 定向构建；标准资源生成默认离线，数据库结构导入按需启用 `schema-import`。前端首屏只同步加载核心、外壳和全局导出文案，生产构建自动检查初始依赖图与包体积预算。

构建、检查、首页和业务负载的性能通过 `cargo xtask check perf` 测量，使用方式见[开发指南](docs/development.md)。源码或工具链变化后，既有测量只能作为历史参考；报告需要对应实际源码、硬件、服务配置和冷暖缓存条件，不能用单次构建耗时或包体积推断首页打开时间。当前改造后的完整恢复和性能验收仍在准备中，不代表已达到正式版发布条件。

## 环境准备

本地开发使用 Windows，需要准备：

- Rust 1.98.0（由 `rust-toolchain.toml` 固定，最低版本为 1.98）；
- MySQL；
- WSL 中的 Redis；
- 需要文件能力时启动 Windows RustFS；
- 前端所需的 Node.js 与 pnpm。

配置从 `config/` 中对应环境的文件加载，并可使用 `APP_` 环境变量覆盖。密码、令牌和证书请使用本机环境变量或密钥文件，不要写入配置样例。

## 启动开发环境

```powershell
$env:APP_ENV = "dev"
cargo xtask data migrate verify
cargo xtask dev
```

`cargo xtask data migrate verify` 校验控制库结构，不修改数据库。需要更新本地数据库时运行 `cargo xtask data migrate up`；租户数据目标可使用 `cargo xtask data migrate verify tenant-data --all` 校验。

`cargo xtask dev` 同时管理 API、Worker 和 Vite，并在后端修改后完成探活再切换版本。按 `Ctrl+C` 可停止整组进程。

排障时仍通过 `cargo xtask dev` 管理 API、Worker 与其进程树；维护二进制的专用操作按职责从
`cargo xtask data --help` 进入，避免绕开运行收据和 ownership 核验。

## 开发与检查

日常修改后运行：

```powershell
cargo xtask check
```

准备联调或交付前运行完整检查：

```powershell
cargo xtask check --full
```

`--scope backend|frontend` 可限制主要检查侧。更多迁移、测试和排障命令见[开发指南](docs/development.md)。

生产构建分别生成 release API、Worker 和前端生产目录，并输出每项产物的路径、大小和 SHA-256。`--plan` 使用同一任务图展示依赖、输入范围和允许写入，不执行任务或写入文件：

```powershell
cargo xtask build --plan
cargo xtask build
```

实际构建会在开始和结束时核对前后端工作树指纹；构建期间来源变化时直接失败，不能把混合来源产物作为成功结果。

## 同步 API 契约

`openapi/openapi.json` 是后端 HTTP 契约快照。接口变化后同步前端派生契约：

```powershell
cargo xtask generate api --write
```

完成同步后，在前端运行消费者检查并进行浏览器联调。请求格式、认证方式和稳定错误码见 [API 指南](docs/api.md)。

## 开发标准资源

标准 CRUD 资源通过资源清单离线生成。预览和检查都只读，只有显式的 `--write` 会更新生成结果：

```powershell
cargo xtask generate resource post
cargo xtask generate resource post --check
cargo xtask generate resource --all --check
cargo xtask generate resource post --write
cargo xtask generate resource post --explain
```

资源清单位于 `catalog/resources/`。Post 和 Notice 可作为标准资源示例；导出、消息发布等特殊行为使用普通 Rust 用例扩展。完整流程见[开发指南](docs/development.md)。

## 文档

- [架构与扩展位置](docs/architecture.md)
- [开发指南](docs/development.md)
- [API 使用](docs/api.md)
- [数据与迁移](docs/data.md)
- [部署与排障](docs/operations.md)

字段、菜单、权限、配置默认值和生成结果分别以 OpenAPI、`catalog/access.toml`、配置结构、资源清单及命令 `--help` 为准。
