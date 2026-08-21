# RyFrame

RyFrame 是面向企业后台的 Rust 2024 服务端，与 RyFrame-Vue3 配套。项目提供认证授权、系统管理、多租户、异步任务、筛选导出、对象存储和可观测性能力。

## 当前边界

- Workspace 固定为 10 个产品 crate 与 2 个工具 crate。
- `ryframe-application` 只包含用例、事务边界和端口。
- `ryframe-db`、`ryframe-tenant-db` 实现 SQL 持久化。
- `ryframe-adapters` 只实现非 SQL 出站能力。
- `ryframe-api` 只负责 HTTP、DTO、OpenAPI 和传输中间件。
- 在线代码生成已经删除，只保留离线 `ryframe-generator` CLI。

依赖方向和每个 crate 的职责见 [架构](docs/architecture.md)。

## 本地开始

本地开发使用 Windows，不使用 Docker 或 WSL 运行应用。需要准备 Rust 1.97、MySQL、WSL 中的 Redis，以及按需启动的 Windows RustFS。

```powershell
$env:APP_ENV = "dev"
cargo run --locked -p ryframe --bin ryframe-migrate
cargo run --locked -p ryframe
```

Worker 单独启动：

```powershell
$env:APP_ENV = "dev"
cargo run --locked -p ryframe --bin ryframe-worker
```

实际配置字段以 `config/` 中的配置结构和环境变量校验为准。不要把密码、令牌或环境绑定数据提交到仓库。

## 常用检查

```powershell
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::redundant_clone
cargo test --locked --workspace
python scripts/check_architecture.py
python scripts/check_permission_routes.py
```

确定性测试必须随代码提交；`.local-tests` 只保存密钥、人工数据、运行结果和环境绑定验收。

## 契约

后端 OpenAPI 的唯一快照是 `openapi/openapi.json`。前端只通过生成的 operation descriptor 调用接口。涉及接口的提交必须同步生成快照并运行前端消费契约检查。

```powershell
cargo run --locked -p ryframe-api --bin export_openapi -- openapi/openapi.json
```

## 文档

- [架构](docs/architecture.md)
- [开发](docs/development.md)
- [API](docs/api.md)
- [数据](docs/data.md)
- [运维](docs/operations.md)

字段、菜单、权限、配置默认值和生成信息分别以 OpenAPI、`catalog/access.toml`、配置结构及命令 `--help` 为准。
