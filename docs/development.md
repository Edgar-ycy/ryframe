# 开发指南

## 环境与配置

本地开发以 Windows 为准。MySQL 和 RustFS 在 Windows 运行，Redis 连接 WSL 中的实例；应用本身直接在 Windows 启动。

选择开发配置后再运行命令：

```powershell
$env:APP_ENV = "dev"
```

配置文件位于 `config/`，环境变量使用 `APP_` 前缀覆盖对应字段。外部服务地址、密码、令牌和证书通过本机环境变量或密钥文件提供。所有可用字段和校验范围以配置结构及启动错误为准。

## 启动与热切换

首次启动先验证数据库结构，再启动开发进程：

```powershell
cargo migrate verify
cargo dev
```

`cargo dev` 启动 Vite、API 和独立 Worker。后端变化会先编译到新目录，并在隔离端口探活；探活成功后才切换正式端口，失败时继续使用上一个可用版本。按 `Ctrl+C` 停止全部子进程。

常用排障入口：

```powershell
cargo run --locked -p ryframe --bin ryframe -- --probe
cargo run --locked -p ryframe --bin ryframe-worker -- --probe
```

探活失败时先检查启动日志，再依次检查数据库结构、Redis、对象存储和端口占用。

## 数据库迁移

控制库使用默认目标：

```powershell
cargo migrate status
cargo migrate verify
cargo migrate up
```

租户数据可操作全部已登记目标，或只操作一个目标：

```powershell
cargo migrate verify tenant-data --all
cargo migrate verify tenant-data --target <目标键>
```

创建新的迁移骨架：

```powershell
cargo migrate new control <迁移名>
cargo migrate new tenant-data <迁移名>
```

生产部署和非生产重建的安全步骤见[数据指南](data.md)与[运维指南](operations.md)。

## 开发标准资源

资源入口默认只预览差异，`--write` 才写入生成结果，`--explain` 可查看从资源清单到页面的调用链：

```powershell
cargo resource --help
cargo resource post
cargo resource post --write
cargo resource post --explain
```

开发新的标准资源时：

1. 在 `catalog/resources/` 增加或修改资源清单。
2. 预览生成差异，确认字段、校验、筛选、排序和权限。
3. 使用 `--write` 更新后端、OpenAPI 和前端派生文件。
4. 在前端补充资源需要的业务交互。
5. 运行 `cargo verify`，再用浏览器验证新增、查询、编辑和删除流程。

Post 和 Notice 可作为标准 CRUD 示例。导出、发布等特殊动作适合保留为自定义强类型用例。

## 开发自定义业务

不能由标准资源表达的流程按以下顺序实现：

1. 在 `ryframe-application::system` 的对应业务域增加用例请求、结果和流程。
2. 若需要数据库操作，在相应 DB 模块实现 Repository；若需要 Redis、对象存储或其他连接，在 adapters 实现端口。
3. 在 `ryframe` 的启动装配中构造并注入实现。
4. 在 `ryframe-api` 增加 DTO、路由和 OpenAPI 描述，或让 Worker 调用应用用例。
5. 增加覆盖业务成功和失败路径的测试。
6. 同步前端契约并联调。

模块选择和请求流见[架构说明](architecture.md)。

## API 与前后端联调

接口变化后运行：

```powershell
cargo api-sync
```

该命令从当前后端代码生成候选 OpenAPI，并刷新前端 operation descriptor。随后进入前端项目执行消费者检查和浏览器 smoke，确认请求、权限、菜单与页面行为一致。

如果只需要重新导出后端快照，可运行：

```powershell
cargo run --locked -p ryframe-api --bin export_openapi -- openapi/openapi.json
cargo run --locked -p ryframe-db --bin export_mysql_snapshot -- sql/ryframe_config.sql
```

## 测试与检查

日常修改使用智能检查，联调完成后使用完整检查：

```powershell
cargo verify
cargo verify --full
```

只运行后端或前端主要检查时可添加 `--scope backend` 或 `--scope frontend`。

运行某个模块的测试：

```powershell
cargo test --locked -p ryframe-application
cargo test --locked -p ryframe-api
```

真实 MySQL 和 Redis 协议测试默认不连接外部服务。准备好隔离的 schema 或 namespace 后显式启用：

```powershell
$env:RYFRAME_MYSQL_INTEGRATION = "1"
cargo test --locked -p ryframe-db --test mysql_real_protocol -- --nocapture

$env:RYFRAME_REDIS_INTEGRATION = "1"
cargo test --locked -p ryframe-adapters --test redis_real_protocol -- --nocapture
```

测试完成后只清理本次测试创建的精确 schema、key 和对象，不清空共享服务。

## 常见问题

- 启动提示迁移不一致：运行 `cargo migrate status` 和 `cargo migrate verify`，本地确认迁移内容后再执行 `cargo migrate up`。
- Worker 未消费任务：确认 API 与 Worker 都使用 `APP_JOBS_MODE=external`，再检查 Worker 健康端口、lease 和数据库连接。
- 前端请求与后端不一致：重新运行 `cargo api-sync`，再执行前端消费者检查。
- Redis 或对象存储不可用：检查 `scope_id`、连接模式、TLS、ownership marker 和服务端口；详细步骤见[运维指南](operations.md)。
