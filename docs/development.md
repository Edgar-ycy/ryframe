# 开发

## 环境

本地开发以 Windows 为准。MySQL 和 RustFS 运行在 Windows，Redis 连接 WSL 实例；应用本身不通过 Docker 或 WSL 启动。环境密钥和测试结果放入忽略的 `.local-tests` 或本机密钥管理中。

## 基本命令

```powershell
cargo dev
cargo verify
cargo verify --full
```

- `cargo dev`：检查工具链后启动 Vite、独立 API 和独立 Worker；启动和每次切换前只执行迁移 `verify`，不会修改数据库。后端变更会在当前服务继续运行时编译到版本化目录，并在隔离端口同时探活 API 与不消费任务的 Worker；任一失败都保留 last-known-good，正式端口启动失败自动恢复旧版本。Windows 用 Job Object 统一回收进程树，按 `Ctrl+C` 停止全部进程。
- `cargo verify`：根据当前变更执行最小安全检查，自动包含后端反向依赖；执行前会按手写产品代码、测试、生成物、迁移、文档和工具输出修改扩散统计，并列出涉及领域与中央热点。预算超限先给出提醒；标准资源变更若仍手改中央注册则直接失败。共享、依赖、CI 和未知变更自动扩大完整门禁。`--scope backend|frontend` 限制主要检查侧；`--full` 下资源生成和消费契约仍会跨前后端验证，避免单侧通过却产生不兼容契约。
- `cargo verify --full`：执行后端、前端、消费契约和浏览器 smoke 的完整本地门禁。
- `cargo api-sync`：从当前后端源码同步候选 OpenAPI 和前端派生文件。
- `cargo migrate verify|up|status`：默认操作控制库；租户数据目标需显式给出 `tenant-data --all` 或 `--target KEY`。
- `cargo migrate new control|tenant-data <迁移名>`：创建只允许追加和 roll-forward 的迁移骨架，并安全注册到对应 Migrator。

这些短命令由仓库内 `xtask` 实现。`cargo xtask ...` 保留给 CI、发布检查和维护任务，不作为普通开发者需要记忆的公共命令。

单次 `cargo verify` 会固定 Workspace 根目录、并发预算和 Cargo target 策略。本地智能门禁默认复用日常开发的 `target`，并尊重外部 `CARGO_TARGET_DIR`；本地显式或自动扩大的完整门禁固定使用 `target/verify/backend`，临时资源 Workspace 使用 `target/verify/resource`。CI 以既有 `CI` 环境变量判定，忽略外部 `CARGO_TARGET_DIR`，分别固定使用 `target/ci/backend` 与 `target/ci/resource`。xtask 自身仍由 Cargo alias 隔离到 `target/xtask-run`。智能后端门禁执行 Clippy 后直接测试，不再额外运行被 Clippy 覆盖的 `cargo check`。若选中的 package 测试能够生成 OpenAPI 或 MySQL 快照，门禁复用同一次 `cargo test` 的快照；仅修改正式快照且未选中生产 package 时，仍执行聚焦的快照导出命令。

迁移冻结不进入日常命令集。维护者在新迁移实现和测试完成后运行内部命令 `cargo xtask migrate freeze`；`cargo verify --full` 和 CI 会拒绝漏冻结，冻结后禁止修改、删除或改名。

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

CI 的稳定内部入口是 `cargo xtask ci plan|preflight|rust-gate|integration|consumer-contract`。
`plan` 复用本地变更分类和反向依赖图，工作流 YAML 只负责 runner、容器、缓存、权限和 job 条件；
Clippy、feature matrix、Workspace test 与快照复用同一个 `rust-gate` target，避免跨 job 重编译。
Windows Cargo 测试默认 `--jobs 4`，其余编译并发由统一的内存预算计算。CI 使用固定版本
`sccache` 的固定版本远端后端仅缓存 Cargo 依赖，不缓存整个 target。前端分别提供
`ci:static`、`ci:unit`、`ci:build`、`ci:browser`，由稳定的 `Required` job 汇总。

Playwright Chromium 只在 browser job 安装，失败时始终上传 trace、截图、视频和报告。真实协议
测试只编译对应 crate；定时、手动与 `v*` 发布标签额外执行真实 API + MySQL + Redis + Chrome
全栈门禁。Windows smoke 只覆盖静态编译、类型与确定性测试，不在 Windows CI 启动 Docker。

## 测试

可确定复现且不含密钥、环境数据的单元、集成、契约和浏览器 smoke 测试必须进入自动化门禁。测试应覆盖成功、失败关闭、租户隔离、权限变化和竞争条件。

九个没有内置单元测试的可执行目标在 Cargo 清单中设置了 `test = false`，只跳过空测试壳的重复链接，不删除测试场景。架构门禁会拒绝在这些目标中静默新增测试；确需就地单测时必须同时恢复测试壳。`cargo verify` 和 CI 仍通过 `clippy --all-targets --all-features` 编译全部二进制，快照工具继续由正式导出命令运行，产品二进制继续由构建、Windows 检查和全栈门禁覆盖；Workspace test 专注于实际单元与集成测试。

MySQL 与 Redis 真实协议测试默认明确跳过，只有分别设置 `RYFRAME_MYSQL_INTEGRATION=1`、`RYFRAME_REDIS_INTEGRATION=1` 才连接外部服务。CI 使用固定镜像执行；本地只连接现有 Windows MySQL 与 WSL Redis，不为测试启动 Docker。MySQL 每个测试创建并精确删除唯一 schema，Redis 每个测试使用唯一 namespace 且只删除测试记录的精确键，禁止 `FLUSH`、`KEYS` 和模糊清理。

```powershell
cargo test --locked -p ryframe-db --test mysql_real_protocol -- --nocapture
cargo test --locked -p ryframe-adapters --test redis_real_protocol -- --nocapture
```

非生产重建只运行纯测试，不要在开发检查中执行真实 `plan` 或 `execute`：

```powershell
cargo test --locked -p ryframe --features destructive-reset --test reset_contract
```

## 代码生成

代码生成只允许离线执行。资源入口默认预览，显式 `--write` 才允许写文件，`--explain` 用于查看资源链路；具体参数以帮助为准。

```powershell
cargo resource --help
cargo resource post
cargo resource post --write
cargo resource post --explain
```

`--write` 在资源文件安全写入后自动刷新候选 OpenAPI；如果后端尚不能编译或契约生成失败，命令会明确说明资源写入已经完成，并提示修复后运行 `cargo api-sync`。底层生成器仍是工具 crate，不进入生产程序。生成结果必须通过快照、重复生成零差异和编译测试。

### 标准资源生产基准

Post 与 Notice 是标准资源生成链路的两个生产基准。字段、校验、筛选、排序、数据权限、CRUD
路径与权限从各自资源 TOML 生成；重复执行生成器必须报告零写入。标准 CRUD 不允许手改中央
Router、页面注册表、权限聚合、应用服务集合或数据库 Repository 注册。Post 导出和 Notice
消息发布等非标准行为各自保留为单一强类型扩展，不把 Config、Dict、Tenant 或 Schedule
强行压入 flat CRUD。字段演进以资源 fixture 和生成契约测试为准，不再维护重复的人工字段清单。

## 前后端联调

涉及 API、字段、权限、菜单或路由时：

1. 更新后端实现与 OpenAPI。
2. 更新前端生成物和调用方。
3. 运行同步 consumer contract。
4. 运行前后端完整门禁与浏览器 smoke。

## 效率采样

`cargo verify` 会把步骤名称、命令、耗时、结果、范围、实际使用的后端与资源 Cargo target 状态和 sccache 状态追加到 `target/verify/metrics.jsonl`。自动门禁按完整成功样本汇总；P50 使用中间两个样本均值，P95 使用保守的 nearest-rank。

需要比较优化前后耗时时，在同一 Windows 环境和同一源码状态下分别采集冷、暖样本，并把命令、缓存条件和结果保存在 `.local-tests/devex/<日期>/`。文档不维护历史跑分表，测试场景也不得为了缩短耗时而删减。

## 注释与所有权

新增产品注释和界面文字使用中文。接口优先接受 `&str`、`&[T]` 和引用计划；批次用 `into_iter()` 消费，避免构造第二份集合。二进制数据使用流或 `Bytes`，只在共享所有权处显式 `Arc::clone`。
