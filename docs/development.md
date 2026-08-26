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
- `cargo verify`：根据双仓 Git 变更执行最小安全检查，自动包含后端反向依赖；执行前会按手写产品代码、测试、生成物、迁移、文档和工具输出修改扩散统计，并列出涉及领域与中央热点。预算超限先给出提醒；标准资源变更若仍手改中央注册则直接失败。共享、依赖、CI 和未知变更自动扩大完整门禁。`--scope backend|frontend` 限制主要检查侧；`--full` 下资源生成和消费契约仍会跨前后端验证，避免单侧通过却产生不兼容契约。
- `cargo verify --full`：执行后端、前端、消费契约和浏览器 smoke 的完整本地门禁。
- `cargo api-sync`：从未提交的后端工作树同步候选 OpenAPI 和前端派生文件，不修改正式来源元数据。
- `cargo api-sync --commit HEAD`：把已提交的正式后端 OpenAPI 同步到前端。
- `cargo migrate verify|up|status`：默认操作控制库；租户数据目标需显式给出 `tenant-data --all` 或 `--target KEY`。
- `cargo migrate new control|tenant-data <迁移名>`：创建只允许追加和 roll-forward 的迁移骨架，并安全注册到对应 Migrator。

这些短命令由仓库内 `xtask` 实现。`cargo xtask ...` 保留给 CI、发布检查和维护任务，不作为普通开发者需要记忆的公共命令。

单次 `cargo verify` 会固定 Workspace 根目录、并发预算和 Cargo target 策略。本地智能门禁默认复用日常开发的 `target`，并尊重外部 `CARGO_TARGET_DIR`；本地显式或自动扩大的完整门禁固定使用 `target/verify/backend`，临时资源 Workspace 使用 `target/verify/resource`。CI 以既有 `CI` 环境变量判定，忽略外部 `CARGO_TARGET_DIR`，分别固定使用 `target/ci/backend` 与 `target/ci/resource`。xtask 自身仍由 Cargo alias 隔离到 `target/xtask-run`。智能后端门禁执行 Clippy 后直接测试，不再额外运行被 Clippy 覆盖的 `cargo check`。若选中的 package 测试能够生成 OpenAPI 或 MySQL 快照，门禁复用同一次 `cargo test` 的快照；仅修改正式快照且未选中生产 package 时，仍执行聚焦的快照导出命令。

迁移冻结属于提交维护动作，不进入日常命令集。维护者在新迁移实现和测试完成后运行内部命令 `cargo xtask migrate freeze`；它只接受尚未进入 `HEAD` 的新迁移，`cargo verify --full` 和 CI 会拒绝漏冻结，冻结后禁止修改、删除或改名。

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

CI 使用固定版本 `sccache` 复用 Rust 编译结果，前端复用 pnpm store 与 Playwright Chromium；真实协议测试只编译对应 crate，不重复执行全 Workspace。前后端分支保护统一绑定稳定的 `Required` job，Windows smoke 只覆盖静态编译、类型与确定性测试，不在 Windows CI 启动 Docker。

## 测试

可确定复现且不含密钥、环境数据的单元、集成、契约和浏览器 smoke 测试必须进入 Git 和 CI。测试应覆盖成功、失败关闭、租户隔离、权限变化和竞争条件。

MySQL 与 Redis 真实协议测试默认明确跳过，只有分别设置 `RYFRAME_MYSQL_INTEGRATION=1`、`RYFRAME_REDIS_INTEGRATION=1` 才连接外部服务。CI 使用固定镜像执行；本地只连接现有 Windows MySQL 与 WSL Redis，不为测试启动 Docker。MySQL 每个测试创建并精确删除唯一 schema，Redis 每个测试使用唯一 namespace 且只删除测试记录的精确键，禁止 `FLUSH`、`KEYS` 和模糊清理。

```powershell
cargo test --locked -p ryframe-db --test mysql_real_protocol -- --nocapture
cargo test --locked -p ryframe-adapters --test redis_real_protocol -- --nocapture
```

非生产重建只运行纯测试，不要在开发检查中执行真实 `plan` 或 `execute`：

```powershell
cargo test --locked -p ryframe --features destructive-reset --bin ryframe-reset
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

### Post 迁移前人工触点基线

2026-08-23 在后端提交 `305a7a7`、前端提交 `321f29e` 上盘点 Post 标准 CRUD。下列是增加字段或排查完整调用链时需要知道的人工触点；OpenAPI、SQL 和 TypeScript 契约派生文件不计入人工文件，但修改后仍需刷新：

- 应用层：`crates/ryframe-application/src/system/post.rs`、`ports/system/post.rs` 及两处 `mod.rs` 注册。
- 数据库层：`crates/ryframe-db/src/entities/post.rs`、`application_ports/system/post.rs`、`repositories/post_repo.rs`，以及 `lib.rs`、`repositories/mod.rs` 注册。
- API 层：`dto/post_dto.rs`、`handlers/post_handler.rs`，以及 DTO、Handler、state、router、router group、OpenAPI 共七处中央接入。
- 权限与初始化：`catalog/access.toml`、控制库 baseline/seeder；字段变化还会刷新 `openapi/openapi.json` 与 `sql/ryframe_config.sql`。
- 前端：`src/views/system/post/index.vue`、`src/generated/resources/post.ts`、`src/router/pageRegistry.ts`；`src/api/modules/post.ts` 中的导出属于明确的 Post 扩展，不计入通用 CRUD 内核。

这份清单是迁移后的反向验收基线：标准资源应只修改一个 TOML，最多增加一个强类型扩展文件；不再要求开发者记住上述中央注册。旧实现删除后保留本节，便于量化人工触点是否真正减少。

## 提交

每次提交只包含一个主题，并在提交前完成受影响的格式化、检查和测试。标题使用 `type(scope): 中文描述`。不要最终 squash，也不要混入用户已有文件。

涉及 API、字段、权限、菜单或路由时：

1. 更新后端实现与 OpenAPI。
2. 更新前端生成物和调用方。
3. 运行同步 consumer contract。
4. 前后端分别提交，发布时使用相同版本和 tag。

## 效率采样

`cargo verify` 会把步骤名称、命令、耗时、结果、范围、提交、工作树状态、实际使用的后端与资源 Cargo target 状态和 sccache 状态追加到 `target/verify/metrics.jsonl`。该文件和步骤日志只保存在 `target`，不提交。自动门禁按完整成功样本汇总；P50 使用中间两个样本均值，P95 使用保守的 nearest-rank。

迁移前自动检查基线（Windows 本机、同一工作树）：2026-08-23 执行 `cargo check --locked -p ryframe-application -p ryframe-db -p ryframe-api --all-targets`，依赖增量检查为 41.16 秒，紧接着的暖缓存检查为 0.70 秒。该数据只代表三层编译反馈，不冒充完整门禁或 CRUD 交付时间；后续采样必须保留命令、提交和冷暖缓存状态。

| 日期 | 提交 | 场景 | 样本数 | 总耗时/P50 | P95 | 结论 |
| --- | --- | --- | ---: | ---: | ---: | --- |
| 2026-08-23 | `f90f0ff` | 首次完整门禁及缓存环境转换 | 1 | 682.4 秒 | 不计算 | 单样本基线 |
| 2026-08-23 | `f90f0ff` | 暖缓存完整门禁 | 1 | 199.7 秒 | 不计算 | 单样本基线 |
| 2026-08-24 | `af2481f` | 新建隔离 Cargo target | 1 | 1608.4 秒 | 不计算 | 未达到 12 分钟 |
| 2026-08-24 | `af2481f` | 暖缓存完整门禁 | 1 | 285.3 秒 | 不计算 | 低于 8 分钟 |
| 2026-08-24 | `ca7a872` | 暖 target、同提交、完整门禁 | 10 | 140.0 秒 | 154.3 秒 | 达到 8 分钟目标；范围 134.5–154.3 秒 |
| 2026-08-24 | `ca7a872` | 全新隔离 target、保留正常 sccache | 1 | 1597.4 秒 | 不计算 | 完整通过，但未达到 12 分钟 |

最终冷样本的 sccache 命中率为 88.9%。主要单步为资源真实 Workspace 456.1 秒、全 Workspace 测试 291.4 秒、最大 feature 测试 250.0 秒、MySQL 快照 152.2 秒和 OpenAPI 快照 114.2 秒。冷缓存目标仍是待优化项，不通过删减测试或把暖缓存冒充冷缓存来关闭。

端到端交付与代码定位属于人工过程，只记录真实观测，不填估算值。每组样本必须让 RyFrame 与 Java 使用相同字段、权限、校验、验收流程和开发环境，并记录从阅读需求到浏览器验收完成的时间。

| 实现侧 | 已完成/目标 | 完整交付时间 | Handler→健康恢复 | 定位 Handler/Service/Port/Repository | 修改文件数 | 手写代码行数 | 状态 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| RyFrame | 0/10 | 待采样 | 待采样 | 待采样 | 待采样 | 待采样 | 待采样 |
| Java | 0/10 | 待采样 | 待采样 | 待采样 | 待采样 | 待采样 | 待采样 |

人工样本由未参与生成器实现的验收人员记录。两侧各积累至少 10 个配对标准 CRUD 样本后再计算 P50/P90/P95；不得用自动编译数据、单次最快结果或推算值代替。在样本充足前，不能宣称已经达到 Java 比例目标。本次暖缓存采样期间后端工作树保持干净；前端工作树状态仅包含原有未跟踪的 `qodana.yaml`，该文件未修改、未提交。

## 注释与所有权

新增产品注释、界面文字和提交描述使用中文。接口优先接受 `&str`、`&[T]` 和引用计划；批次用 `into_iter()` 消费，避免构造第二份集合。二进制数据使用流或 `Bytes`，只在共享所有权处显式 `Arc::clone`。
