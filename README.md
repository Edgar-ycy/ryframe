# RyFrame

RyFrame 是面向企业后台的 Rust 2024 服务端，与 RyFrame-Vue3 配套。项目提供认证授权、系统管理、多租户、异步任务、筛选导出、对象存储和可观测性能力。

## 当前边界

- Workspace 固定为 10 个产品 crate 与 2 个工具 crate。
- `ryframe-application` 只包含用例、事务边界和端口。
- `ryframe-db`、`ryframe-tenant-db` 实现 SQL 持久化。
- `ryframe-adapters` 只实现非 SQL 出站能力。
- `ryframe-api` 只负责 HTTP、DTO、OpenAPI 和传输中间件。
- 在线代码生成已经删除，只保留由 `cargo resource` 驱动的离线资源生成。

依赖方向和每个 crate 的职责见 [架构](docs/architecture.md)。

## 本地开始

本地开发使用 Windows，不使用 Docker 或 WSL 运行应用。需要准备 Rust 1.97、MySQL、WSL 中的 Redis，以及按需启动的 Windows RustFS。

```powershell
$env:APP_ENV = "dev"
cargo migrate verify
cargo dev
```

`cargo migrate verify` 默认校验控制库；需要更新本地结构时显式运行 `cargo migrate up`。租户数据目标使用 `cargo migrate verify tenant-data --all`。`cargo dev` 统一管理 API、Worker 与 Vite。

`cargo dev` 已使用 `jobs.mode=external` 同时管理独立 Worker。只在排障时手工启动 Worker，并确保 API 也使用 external 模式：

```powershell
$env:APP_ENV = "dev"
$env:APP_JOBS_MODE = "external"
cargo run --locked -p ryframe --bin ryframe-worker
```

实际配置字段以 `config/` 中的配置结构和环境变量校验为准。不要把密码、令牌或环境绑定数据提交到仓库。

## 常用检查

```powershell
cargo verify
cargo verify --full
```

日常检查使用 `cargo verify`，它根据前后端 Git 变更选择受影响包、反向依赖或前端检查画像；依赖、CI、共享配置和未知变更会自动扩大为完整门禁。提交前使用 `cargo verify --full`，覆盖后端测试、Cargo feature 组合、前端消费契约和浏览器 smoke。`--scope backend|frontend` 可限制主要检查侧，但完整门禁中的资源生成和消费契约仍会跨仓验证。底层 `cargo xtask ...` 是 CI 与维护者使用的内部入口，普通开发不需要记忆。

确定性测试必须随代码提交；`.local-tests` 只保存密钥、人工数据、运行结果和环境绑定验收。

## 契约

后端 OpenAPI 的唯一快照是 `openapi/openapi.json`。前端只通过生成的 operation descriptor 调用接口。涉及接口的提交必须同步生成快照并运行前端消费契约检查。

```powershell
cargo run --locked -p ryframe-api --bin export_openapi -- openapi/openapi.json
cargo api-sync
cargo api-sync --commit HEAD
```

无参数的 `cargo api-sync` 从当前后端工作树导出候选契约，并刷新前端派生文件，不修改 `openapi/source.json` 中的正式来源。正式同步要求后端 OpenAPI 已提交，`--commit` 可接收 `HEAD` 或其他 Git 引用，并会固定为完整提交 SHA。

## 标准资源

```powershell
cargo resource post
cargo resource post --write
cargo resource post --explain
```

默认只预览可读 diff；`--write` 安全写入生成资产，并刷新当前工作树的候选 OpenAPI 与前端派生契约；`--explain` 输出从清单到页面的完整调用链。资源清单位于 `catalog/resources/`，复杂业务继续放在普通 Rust 扩展中。

## 文档

- [架构](docs/architecture.md)
- [开发](docs/development.md)
- [API](docs/api.md)
- [数据](docs/data.md)
- [运维](docs/operations.md)

字段、菜单、权限、配置默认值和生成信息分别以 OpenAPI、`catalog/access.toml`、配置结构及命令 `--help` 为准。
