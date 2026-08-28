# 开发指南

## 环境与配置

本地开发以 Windows 为准。MySQL 和 RustFS 在 Windows 运行，Redis 连接 WSL 中的实例；应用本身直接在 Windows 启动。

选择开发配置后再运行命令，例如 `$env:APP_ENV = "dev"`。配置文件位于 `config/`，环境变量使用 `APP_` 前缀覆盖对应字段；外部服务地址、密码、令牌和证书通过本机环境变量或密钥文件提供，所有可用字段和校验范围以配置结构及启动错误为准。

## 启动与热切换

首次启动先验证数据库结构，再启动开发进程：

```powershell
cargo migrate verify
cargo dev
```

`cargo dev` 启动 Vite、API 和独立 Worker。后端变化会先编译到按会话和代次隔离的新目录，并在隔离端口探活；探活成功后才切换正式端口，失败时继续使用上一个可用版本。再次启动时会清理上次崩溃留下的暂存目录，并校验清单、成对二进制与运行输入快照，恢复最近的完整版本后再后台构建当前源码。按 `Ctrl+C` 停止全部子进程；若 `xtask` 自身变化，命令以退出码 `75` 提示重新运行 `cargo dev`。

常用排障入口：

```powershell
cargo run --locked -p ryframe --no-default-features --features bin-api,runtime-swagger-ui --bin ryframe -- --probe
cargo run --locked -p ryframe --no-default-features --features bin-worker --bin ryframe-worker -- --probe
```

## 数据库迁移

控制库使用默认目标；租户数据可操作全部已登记目标或单个目标；新迁移必须用对应命令创建骨架：

```powershell
cargo migrate status
cargo migrate verify
cargo migrate up
cargo migrate verify tenant-data --all
cargo migrate verify tenant-data --target <目标键>
cargo migrate new control <迁移名>
cargo migrate new tenant-data <迁移名>
```

生产部署和非生产重建步骤见[数据指南](data.md)与[运维指南](operations.md)。

## 开发标准资源

资源入口默认只预览差异；`--check` 只读比较并在存在差异时返回失败，`--write` 才写入生成结果，`--explain` 可查看从资源清单到页面的调用链：

```powershell
cargo resource --help
cargo resource post
cargo resource post --check
cargo resource --all --check
cargo resource post --write
cargo resource post --explain
```

`cargo resource --all --check` 会校验全部受管后端、前端资产和 ownership 清单，适合在提交前确认重复生成零差异。该命令不会创建临时生成文件、刷新 OpenAPI 或连接数据库；发现差异后，先按资源预览，再显式执行对应的 `--write`。

开发新的标准资源时：

1. 在 `catalog/resources/` 增加或修改资源清单。
2. 预览生成差异，确认字段、校验、筛选、排序和权限。
3. 使用 `--write` 更新后端、OpenAPI 和前端派生文件。
4. 运行 `cargo resource --all --check` 确认资源目录零差异。
5. 在前端补充资源需要的业务交互。
6. 运行 `cargo verify`，再用浏览器验证新增、查询、编辑和删除流程。

Post 和 Notice 可作为标准 CRUD 示例。导出、发布等特殊动作适合保留为自定义强类型用例。

## 开发自定义业务

不能由标准资源表达的流程按以下顺序实现：

1. 在 `ryframe-application::system` 的对应业务域增加用例请求、结果和流程。
2. 若需要数据库操作，在相应 DB 模块实现 Repository；若需要 Redis、对象存储或其他连接，在 adapters 实现端口。
3. 在 `ryframe` 的启动装配中构造并注入实现。
4. 在 `ryframe-api` 增加 DTO、路由和 OpenAPI 描述，或让 Worker 调用应用用例。
5. 增加覆盖业务成功和失败路径的测试。
6. 同步前端契约并联调；模块选择和请求流见[架构说明](architecture.md)。

## API 与前后端联调

接口变化后运行 `cargo api-sync`，从当前后端代码生成候选 OpenAPI 并刷新前端 operation descriptor；随后进入前端项目执行消费者检查和浏览器 smoke，确认请求、权限、菜单与页面行为一致。只重新导出后端快照时运行：

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

完整集成门禁还会运行 Redis TLS、S3 HTTPS 和 OTLP HTTPS 三个 AWS-LC 真实出站测试。Windows 本地需有 `python`、`openssl`，并在 `127.0.0.1` 启动隔离的 MySQL 与 Redis；Redis 建议使用 WSL 独立实例和数据库 15。门禁只接受回环 Redis，不执行 `KEYS`、`SCAN` 或 `FLUSH`，测试 key 由唯一 `scope_id` 隔离并精确删除。运行入口与 CI 相同：

```powershell
$env:RYFRAME_MYSQL_INTEGRATION = "1"
$env:RYFRAME_REDIS_INTEGRATION = "1"
$env:RYFRAME_REDIS_DATABASE = "15"
$env:RYFRAME_INTEGRATION_RUN_ID = "local-aws-lc"
cargo xtask ci integration
```

TLS fixture 会生成两日有效的临时 CA，在动态回环端口启动 Redis TLS 代理和 HTTPS 服务；三个测试结束或失败后都会停止监听并删除临时证书。日志默认保存在 `.local-tests/integration/tls/<run-id>/`，历史目录不会覆盖；可用 `RYFRAME_TLS_ARTIFACT_DIR` 指定日志根目录。CI 对成功和失败运行都上传 14 天，失败摘要会输出每个已运行测试的最近日志。`RYFRAME_REDIS_HOST` 不是 `127.0.0.1`、`::1` 或 `localhost` 时门禁直接拒绝启动，避免误连共享 Redis。

## 开发反馈性能测量

DevEx 测量必须显式选择 suite、工作负载变体、运行次数与冷暖缓存状态，例如 `cargo xtask devex run --suite rust-cold-build --variant api --runs 20 --cache cold`。Rust suite 的变体为 `api`、`worker`、`migrate` 或 `workspace`；`cargo-dev-save` 的变体直接选择 `config-only`、`api-only`、`worker-only`、`shared-runtime`、`locales`、`migration-only`、`resource-manifest` 或 `cancellation`，不依赖调用方预设环境变量；resource generator 使用 `all`、`post` 或 `notice`，resource gate 使用 `auto`，Rust gate 和前端 suite 使用 `default`。普通 suite 至少采样 5 次，Rust suite 及会触发编译的保存场景至少采样 20 次。`rust-incremental` 会对所选目标的代表性源码执行一次可还原编辑，命令成功或失败后均原子还原。

`cargo-dev-save` 每个样本会先在独立 session、动态端口和隔离运行输入中构建并以只读 probe 模式启动 LKG；MySQL、Redis 或对象存储未就绪时，前置检查直接失败且不产出样本。计时从代表性源码的原子保存开始，完整经过真实 watcher debounce、后台 build/verify、候选 probe、LKG promotion 或 `VerifiedNoRestart`，直到服务再次 ready；前置检查、源码还原和进程清理不计入耗时，也不会自动执行迁移升级。`samples.jsonl` 同时记录保存到就绪耗时、该保存周期实际发起的 Cargo 调用数和就绪类型；`config-only` 必须经过真实 LKG 复用、probe 与切换且 Cargo 调用数为 0。`cold` 表示干净 target 上的首次保存，`warm` 表示复用同一 target 的成熟保存，但每次样本仍使用新的运行 session。

`cancellation` 不启动 LKG，而是在样本专用冷 target 中启动真实 API Cargo 构建，确认 Job Object 中已出现编译后代后产生新源码代次。耗时从 watcher 可观察的原子保存开始，到整棵 Cargo/rustc/build-script 进程树回收且活跃进程数归零为止；每个正式分布至少记录 20 个样本。成功的冷样本在先写入测量记录后删除已验证边界内的隔离 target，失败样本保留 target 用于诊断。

suite 固定为 `rust-cold-build`、`rust-incremental`、`cargo-dev-save`、`resource-generator`、`resource-gate`、`rust-gate`、`rust-sccache`、`frontend-fast` 和 `frontend-build`。`rust-gate` 精确执行 `cargo xtask ci rust-gate`，不会使用旧的完整 verify 代替；它与 `rust-sccache` 都在预热专用缓存后采集 sccache 前后统计，并让每个正式样本使用独立 Cargo target。成功样本在记录后删除隔离 target，避免把 Cargo no-op 算成命中或让 20 轮门禁无限占用磁盘。

产物只写入 `.local-tests/devex/<日期>/<run-id>/`：`metadata.json` 记录提交、dirty worktree 内容指纹、Cargo、Rust、sccache 可执行版本、target、features、jobs、环境白名单哈希、cache state、可比较的执行面 `compile_surface_fingerprint` 与单独的输入指纹，`samples.jsonl` 保存样本，`summary.json` 和 `summary.md` 保存 P50/P95。正式对比必须使用 `cargo xtask devex paired --base-backend <基线-worktree> --candidate-backend <候选-worktree> ...`，两个测量 worktree 与当前 worktree 必须彼此独立；前端参与的 suite 还必须提供两份独立前端 worktree。runner 按 A-B-B-A 交错记录 arm、pair、全局 order 与每侧源码指纹。完成后可分别 `summarize`，`compare --base <基线-run> --candidate <候选-run>` 只接受同一 comparison id、相同 sccache 版本且样本完整、次序可审计的 paired 结果。

## 资源门禁与编译缓存

`cargo xtask ci resource-gate --frontend-dir ../ryframe-vue3` 直接从 CI 的 base/head SHA 读取资源变化、关系闭包和 ownership，不接收流水线拼接的资源名。缺少合法 base、变更面过大、删除或重命名无法归属，以及 Cargo、toolchain、模板、CI 或架构策略变化时都会自动执行完整门禁。定向模式默认关闭；只有 `scripts/resource_gate_replay.py` 在隔离 worktree 中用至少 20 个真实变更案例证明定向与完整门禁零分歧后，CI 才能设置受控的激活标记。没有回放证据时保持完整回退是预期行为。

Rust CI 关闭 incremental，并为每个 job 保存 sccache JSON 统计，不缓存整个 target。`SCCACHE_BASEDIRS` 目前只用于定时或手动的 AWS-LC 双绝对路径 canary；canary 要求缓存错误为零、warm 命中率至少 80%、不可缓存请求至少减少 50%，且 warm 构建确有耗时改善。达到这些条件前，不把该路径归一化配置扩展到普通 Rust job。

## 常见问题

- 启动提示迁移不一致：运行 `cargo migrate status` 和 `cargo migrate verify`，本地确认迁移内容后再执行 `cargo migrate up`。
- Worker 未消费任务：确认 API 与 Worker 都使用 `APP_JOBS_MODE=external`，再检查 Worker 健康端口、lease 和数据库连接。
- 前端请求与后端不一致：重新运行 `cargo api-sync`，再执行前端消费者检查。
- Redis 或对象存储不可用：检查 `scope_id`、连接模式、TLS、ownership marker 和服务端口；详细步骤见[运维指南](operations.md)。
