# RyFrame

当前为 `0.x` 开发版本，安装和联调只支持当前 API、配置、任务载荷与数据库基线；请使用全新隔离数据库，不直接覆盖旧开发库。

当前提供用户、角色、权限、组织、产品套餐、多租户、配置迁移、跨库迁移、监控、消息、调度、导入和导出，以及个人资料、密码、头像和登录会话管理。套餐可以发布不含可选能力的版本，租户开通、基础管理与配额管理照常使用。通知公告和租户主动消息发布可按套餐启停；关闭不删除历史数据，也不关闭系统通知、调度、导入导出或备份恢复。

RyFrame 是面向企业后台的 Rust 2024 服务端，与 RyFrame-Vue3 配套使用。它提供认证授权、系统管理、多租户、异步任务、筛选导出、对象存储和可观测性能力。

项目运行时以 Rust 为唯一业务实现语言。API、Worker、迁移、重建和业务生成器均有独立 Rust 二进制；`cargo xtask` 只用于 RyFrame 自身的检查、构建、CI、性能、恢复和发布维护。

## 当前项目状态

框架维护检查默认只检查后端：`cargo xtask check`；需要前后端完整检查时使用 `cargo xtask check --scope all --full`。前端目录可在 `config/tools.toml` 的 `workspace.frontend_dir` 配置，详见[开发指南](docs/development.md#框架维护检查)。

API、Worker、迁移和维护程序按 feature 定向构建；标准资源生成默认离线，数据库结构导入按需启用 `schema-import`。前端首屏只同步加载核心、外壳和全局导出文案，生产构建自动检查初始依赖图与包体积预算。

构建、检查、首页和业务负载的性能通过 `cargo xtask check perf` 测量；需要写入隔离环境的性能身份准备从 `cargo xtask data performance-identities` 进入，使用方式见[开发指南](docs/development.md)。源码或工具链变化后，既有测量只能作为历史参考；报告需要对应实际源码、硬件、服务配置和冷暖缓存条件，不能用单次构建耗时或包体积推断首页打开时间。当前改造后的完整恢复和性能验收仍在准备中，不代表已达到正式版发布条件。

## 环境准备

本地开发使用 Windows，需要准备：

- Rust 1.98.1（由 `rust-toolchain.toml` 固定，最低版本为 1.98）；
- MySQL 8.0.16 或更新版本（不使用 MariaDB 替代）；
- Redis；
- 需要文件能力时配置本地存储，或连接 RustFS、MinIO、S3 兼容服务；
- 前端所需的 Node.js 与 pnpm。

配置从 `config/` 中对应环境的文件加载，并可使用 `APP_` 环境变量覆盖。密码、令牌和证书请使用本机环境变量或密钥文件，不要写入配置样例。

MySQL、Redis 和 RustFS 可以运行在满足版本和协议要求的任意平台，也可以连接远程实例，不限定 Windows 或 WSL。

## 启动开发环境

新项目默认单租户，身份入口固定使用系统租户，登录页不选择租户。需要多租户时显式设置 `multi_tenancy.enabled = true` 或 `APP_MULTI_TENANCY_ENABLED=true`；已有部署的显式选择继续生效。单租户仍可使用配置迁移与数据维护，租户和套餐管理仅在多租户模式开放。

```powershell
$env:APP_ENV = "dev"
# 先创建空数据库并配置连接，将管理员初始密码保存在这个 UTF-8 文件中
$env:RYFRAME_INIT_ADMIN_PASSWORD_FILE = ".local-tests/secrets/admin-password.txt"
cargo migrate -- control up
cargo migrate -- control verify
cargo serve
```

`cargo migrate -- control up` 初始化或升级已明确选择的控制库；`verify` 只校验，不写数据库。管理员初始密码必须显式提供并符合密码规则；普通初始用户 `user`、新建及导入的普通用户初始密码为 `123456`。所有新账号首次登录必须修改密码，改密前不能访问业务功能。重复迁移不覆盖已有密码或改密状态。租户库必须明确指定配置中的目标键，例如 `cargo migrate -- tenant-data verify --target <目标键>`。

需要交给数据库工具导入时，运行 `cargo migrate -- control export-sql --output .local-tests/control-init.sql --write` 生成完整初始化 SQL，再导入空库并执行 `control verify`。源码中的审阅快照不能代替正式导出文件。详见[数据库初始化](docs/development.md#数据库迁移)。

`cargo serve` 只启动后端 API。需要 Worker 时在另一个终端运行 `cargo worker`；前端在 `ryframe-vue3` 仓库中单独运行 `corepack pnpm dev`。

非生产重建通过 `cargo reset -- <参数>` 执行。备份、恢复、性能和发布演练属于框架维护流程，按运维文档使用 `cargo xtask`。

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

维护和验收统一从 `cargo xtask` 进入，不直接把 `tools/` 中的文件当作用户命令。具体参数以命令 `--help` 与对应使用文档为准。

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

## 创建自己的业务 crate

RyFrame 不内置示例业务 crate。使用正式脚手架在 `crates/` 下按业务域创建一个或多个普通库 crate：

```powershell
cargo generate -- new-business order-business --write
cargo generate-lockfile --offline
cargo generate -- resource --package order-business
cargo generate -- resource --package order-business --model Order --write
```

创建业务 crate 与生成资源默认都预览，只有 `--write` 写入；新增 crate 后显式刷新锁文件。创建负责目录、元数据、SDK、工作区与进程装配，资源生成负责模型对应的 CRUD、迁移和 ownership。业务模型使用 Rust `ResourceModel` 维护，手写代码位于 `resources/` 与 `extensions/`。默认生成器离线，数据库导入才启用 `schema-import`。完整结构和代码见[开发指南](docs/development.md#创建业务-crate)。

## 文档

- [架构与扩展位置](docs/architecture.md)
- [开发指南](docs/development.md)
- [API 使用](docs/api.md)
- [数据与迁移](docs/data.md)
- [部署与排障](docs/operations.md)

字段、菜单、权限、配置默认值和生成结果分别以 OpenAPI、`catalog/access.toml`、配置结构、资源清单及命令 `--help` 为准。
