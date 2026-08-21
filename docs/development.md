# 开发

## 环境

本地开发以 Windows 为准。MySQL 和 RustFS 运行在 Windows，Redis 连接 WSL 实例；应用本身不通过 Docker 或 WSL 启动。环境密钥和测试结果放入忽略的 `.local-tests` 或本机密钥管理中。

## 基本命令

```powershell
cargo fmt --all
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::redundant_clone
cargo test --locked --workspace
```

架构和生成资产：

```powershell
python scripts/check_architecture.py
python scripts/check_permission_routes.py
python scripts/check_deployment_assets.py
python scripts/check_supply_chain.py
cargo run --locked -p ryframe-api --bin export_openapi -- openapi/openapi.json
cargo run --locked -p ryframe-db --bin export_mysql_snapshot -- sql/ryframe_config.sql
```

OpenAPI 和 SQL 快照必须由正式命令生成，不手工编辑。

## 测试

可确定复现且不含密钥、环境数据的单元、集成、契约和浏览器 smoke 测试必须进入 Git 和 CI。测试应覆盖成功、失败关闭、租户隔离、权限变化和竞争条件。

非生产重建只运行纯测试，不要在开发检查中执行真实 `plan` 或 `execute`：

```powershell
cargo test --locked -p ryframe --features destructive-reset --bin ryframe-reset
```

## 代码生成

代码生成只允许离线 CLI。默认 dry-run，显式 `--write` 才写文件，默认不覆盖；具体参数以帮助为准。

```powershell
cargo run --locked -p ryframe-generator -- --help
```

模板生成 Repository 到 `ryframe-db`，生成用例到 `ryframe-application`；生成结果必须通过 golden 和编译测试。

## 提交

每次提交只包含一个主题，并在提交前完成受影响的格式化、检查和测试。标题使用 `type(scope): 中文描述`。不要最终 squash，也不要混入用户已有文件。

涉及 API、字段、权限、菜单或路由时：

1. 更新后端实现与 OpenAPI。
2. 更新前端生成物和调用方。
3. 运行同步 consumer contract。
4. 前后端分别提交，发布时使用相同版本和 tag。

## 注释与所有权

新增产品注释、界面文字和提交描述使用中文。接口优先接受 `&str`、`&[T]` 和引用计划；批次用 `into_iter()` 消费，避免构造第二份集合。二进制数据使用流或 `Bytes`，只在共享所有权处显式 `Arc::clone`。
