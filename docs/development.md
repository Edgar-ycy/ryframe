# 开发指南

当前契约生成使用 `core/system/platform/monitor` 四个领域。套餐能力目录可以为空，开发新业务能力时仍需明确登记 capability，未知能力继续被拒绝。标准 CRUD 的生成与幂等检查保持不变。

## 环境与配置

本地开发以 Windows 为准，MySQL 和 RustFS 在 Windows 运行，Redis 连接 WSL 中的实例，应用直接在 Windows 启动。安装 rustup 后，在项目目录运行 `rustup show` 安装并选择固定的 Rust 1.98.0、rustfmt 和 Clippy；项目使用 Rust 2024 edition，最低 Rust 版本为 1.98，本地、CI 和生产构建镜像使用同一固定工具链。
选择开发配置后再运行命令，例如 `$env:APP_ENV = "dev"`。配置文件位于 `config/`，环境变量使用 `APP_` 前缀覆盖对应字段；外部服务地址、密码、令牌和证书通过本机环境变量或密钥文件提供，所有可用字段和校验范围以配置结构及启动错误为准。
## 启动与热切换

首次启动先验证数据库结构，再启动开发进程：
```powershell
cargo xtask data migrate verify
cargo xtask dev
```
`cargo xtask dev` 启动 Vite、API 和独立 Worker。后端变化会先编译到按会话和代次隔离的新目录，并在隔离端口探活；探活成功后才切换正式端口，失败时继续使用上一个可用版本。再次启动时会清理上次崩溃留下的暂存目录，并校验清单、成对二进制与运行输入快照，恢复最近的完整版本后再后台构建当前源码。按 `Ctrl+C` 停止全部子进程；若 `xtask` 自身变化，命令以退出码 `75` 提示重新运行 `cargo xtask dev`。

保存后的动作由整批路径共同决定；同一批包含多类变化时会合并为覆盖全部变化的计划：

| 变化范围 | `cargo xtask dev` 动作 |
|---|---|
| 当前开发运行配置 | 不调用 Cargo；先比较去注释、脱敏后的配置快照与内存密钥投影。语义相同的保存保持当前 API/Worker 就绪；存在实际差异时复用上一可用二进制，以原子配置快照探活后成对切换 |
| 本地化 catalog | 重新构建 API 和 Worker，因为默认文案包含编译期资源 |
| API crate 或 API 组合入口 | 只构建 API，复用上一可用 Worker，随后成对探活和切换 |
| Worker 入口 | 只构建 Worker，复用上一可用 API，随后成对探活和切换 |
| application、kernel、auth、config、adapters、DB 或共享 boot | 重新构建 API 和 Worker |
| 控制库迁移或访问目录 | 只构建 migrate，复用上一可用 API/Worker，执行控制库 verify 后成对探活和切换；不自动升级 |
| 租户迁移 | 只构建 migrate，并验证当前配置中明确登记的本地租户目标；不重启服务 |
| migrate 入口 | 只构建 migrate 并独立 verify；不重启服务 |
| 资源清单或生成器 | 执行只读 `cargo xtask generate resource --all --check`；发现漂移即失败，不重启服务 |
| Cargo、toolchain、`build.rs` 或本地 vendor | 重新规划并构建 API、Worker 和 migrate |
| 未识别的后端文件 | 保守地重新构建 API 和 Worker |
| `xtask` 自身 | 退出码 `75`，提示开发者重新运行 `cargo xtask dev` |

常用排障从 `cargo xtask check doctor` 开始；运行中的 API 和 Worker 探针由 `cargo xtask dev` 统一收集并记录，维护操作通过 `cargo xtask data --help` 选择明确子操作。
## 数据库迁移

控制库使用默认目标；租户数据可操作全部已登记目标或单个目标；新迁移必须用对应命令创建骨架：

```powershell
cargo xtask data migrate status
cargo xtask data migrate verify
cargo xtask data migrate up
cargo xtask data migrate verify tenant-data --all
cargo xtask data migrate verify tenant-data --target <目标键>
cargo xtask data migrate new control <迁移名>
cargo xtask data migrate new tenant-data <迁移名>
```
生产部署和非生产重建步骤见[数据指南](data.md)与[运维指南](operations.md)。
## 开发标准资源

资源入口默认只预览差异；`--check` 只读比较并在存在差异时返回失败，`--write` 才写入生成结果，`--explain` 可查看从资源清单到页面的调用链：

```powershell
cargo xtask generate --help
cargo xtask generate resource post
cargo xtask generate resource post --check
cargo xtask generate resource --all --check
cargo xtask generate resource post --write
cargo xtask generate resource post --explain
```
`cargo xtask generate resource --all --check` 会校验全部受管后端、前端资产和 ownership 清单，适合在提交前确认重复生成零差异。该命令不会创建临时生成文件、刷新 OpenAPI 或连接数据库；发现差异后，先按资源预览，再显式执行对应的 `--write`。Post 和 Notice 可作为标准 CRUD 示例；导出、发布等特殊动作保留为自定义强类型用例。开发新资源时：

1. 在 `catalog/resources/` 编写资源清单，预览并确认字段、校验、筛选、排序和权限。
2. 使用 `--write` 更新后端、OpenAPI 和前端派生文件，再以 `cargo xtask generate resource --all --check` 确认零差异。
3. 补充前端业务交互，执行 `cargo xtask check`，用浏览器验证新增、查询、编辑和删除。

租户资源生成同时更新建表迁移和复制目录；路由键须与菜单键一致。需要验证完整 Device 链路时，在隔离后端工作树执行 `cargo xtask check recovery fixture --output-dir <后端根目录>/.local-tests/device-fixture --write`。该入口固定当前后端与 `--frontend-dir` 选择的前端工作树，在新建隔离工作树生成资源并验证只读幂等性，不启动外部服务；随后给隔离工作树显式配置全栈测试资源，并以 `RYFRAME_E2E_FIXTURE=device` 运行真实浏览器验收。工作树收据记录源代码与生成内容指纹，失败证据保留在输出目录。
## 开发自定义业务

不能由标准资源表达的流程按以下顺序实现：

1. 在 `ryframe-application::system` 对应业务域编写用例；SQL Repository 在 DB 模块实现，Redis、对象存储等端口在 adapters 实现。
2. 在 `ryframe` 启动装配中注入实现；在 API 层增加 DTO、路由和 OpenAPI 描述，或由 Worker 调用用例。
3. 覆盖业务成功和失败路径的测试，同步前端契约并联调；模块选择和请求流见[架构说明](architecture.md)。
## API 与前后端联调

接口变化后运行 `cargo xtask generate api --write`，从当前后端代码生成候选 OpenAPI 并刷新前端 operation descriptor；随后进入前端项目执行消费者检查和浏览器 smoke，确认请求、权限、菜单与页面行为一致。OpenAPI 与数据库结构快照均由对应的生成或迁移维护入口产生，不直接运行内部二进制。
## 生产构建

`cargo xtask build` 从同一构建计划依次运行三个任务：关闭默认 feature 后分别定向构建 API 和 Worker，再通过 Corepack 构建前端生产目录。每项完成后输出 Cargo 实际报告的可执行文件或完整前端目录的路径、字节数和 SHA-256；`--profile dev` 只改变构建 profile，任务边界保持不变。

`cargo xtask build --plan` 渲染这份计划的依赖、有效参数、输入范围、编译覆盖和允许写入，不启动 Cargo、Corepack 或服务，也不创建目录、缓存和报告。实际执行在首个任务前和全部任务后核对前后端工作树指纹；任一来源在构建期间变化都会失败，已生成文件不能作为同源成功产物。
## 测试与检查

日常修改使用智能检查，联调完成后使用完整检查：

```powershell
cargo xtask check
cargo xtask check --full
```
只运行后端或前端主要检查时可添加 `--scope backend` 或 `--scope frontend`。

模块测试使用 `cargo test --locked -p <包名>`，例如 `ryframe-application` 或 `ryframe-api`。

真实 MySQL 和 Redis 协议测试默认不连接外部服务；需要单独诊断时，显式启用对应的 `RYFRAME_MYSQL_INTEGRATION` 或 `RYFRAME_REDIS_INTEGRATION`，再运行 `mysql_real_protocol` 或 `redis_real_protocol` 测试目标。完整集成使用下面的统一入口。

完整集成门禁会验证 MySQL required TLS，以及 Redis TLS、S3 HTTPS 和 OTLP HTTPS 三个 AWS-LC 真实出站测试。Windows 本地需有 `python`、`openssl`，并在 `127.0.0.1` 启动隔离的 MySQL 与 Redis；Redis 建议使用 WSL 独立实例和数据库 15。门禁只接受回环 Redis，不执行 `KEYS`、`SCAN` 或 `FLUSH`，测试 key 由唯一 `scope_id` 隔离并精确删除。运行入口与 CI 相同：

```powershell
$env:RYFRAME_MYSQL_INTEGRATION = "1"
$env:RYFRAME_MYSQL_TLS_INTEGRATION = "1"
$env:RYFRAME_REDIS_INTEGRATION = "1"
$env:RYFRAME_REDIS_DATABASE = "15"
$env:RYFRAME_INTEGRATION_RUN_ID = "local-aws-lc"
cargo xtask check ci integration
```

TLS fixture 会生成两日有效的临时 CA，在动态回环端口启动 Redis TLS 代理和 HTTPS 服务；相关测试结束或失败后都会停止监听并删除临时证书。日志默认保存在 `.local-tests/integration/tls/<run-id>/`，历史目录不会覆盖；可用 `RYFRAME_TLS_ARTIFACT_DIR` 指定日志根目录。CI 对成功和失败运行都上传 14 天，失败摘要会输出每个已运行测试的最近日志。`RYFRAME_REDIS_HOST` 不是 `127.0.0.1`、`::1` 或 `localhost` 时门禁直接拒绝启动，避免误连共享 Redis。

真实全栈的 API 与 external Worker 每次分别由私有长驻监督进程托管。Windows 监督进程先把自身加入启用关闭即终止的私有 Job Object，Unix 监督进程先建立独立 session 和进程组，再启动产品代码；进程树收据绑定 scope、启动操作以及监督进程和产品进程的创建身份。停止与故障注入使用不同控制收据，保留产品真实退出码；正常停止先请求产品退出并等待宽限，随后才回收完整 Job 或进程组。Windows 当前没有可继承的温和控制台通道，正常停止触发的 `TerminateProcess` 会如实记录为强制终止。控制操作只有在完整进程树停止且监听端口释放后才完成，不会按 PID 猜测或影响未登记进程。
## 开发反馈性能测量

需要测量保存反馈、编译、资源门禁或前端构建时，先用 `cargo xtask check --help` 查看性能子任务，再通过统一入口执行和汇总：

```powershell
cargo xtask check perf run --suite <suite> --variant <name> --runs <次数> --cache <cold|warm>
cargo xtask check perf paired --base-backend <基线目录> --candidate-backend <候选目录> --suite <suite> --variant <name> --runs <次数> --cache <cold|warm>
cargo xtask check perf summarize <日期/run-id>
cargo xtask check perf compare --base <日期/run-id> --candidate <日期/run-id>
```

稳定版准备的原始 B0（后端 `815c5eafb09d4b493319d255fb7ab88ddd02c8b6`、前端 `0087ea2ecf62530d042b9e52f5c950fb34c66d78`）尚未提供当前命令路由。执行任一 B0/B1 suite 时，在后端 B0 上创建独立工作树，应用当前仓库的 `xtask/assets/baseline-adapters/stable-readiness-b0-v1.patch` 并提交，然后将该工作树传给 `--base-backend`，同时将精确、干净的前端 B0 传给 `--base-frontend`，并指定 `--baseline-contract legacy-stable-readiness-b0-v1`。原始 B0 源码保持不变；协调器会在测量前核对两端提交及干净状态、适配提交的直接父提交、补丁字节与摘要、适配树，以及只修改 `xtask/src/cli.rs` 的范围，任一不符即停止。前端快速检查在 B0 执行真实 `check:fast`、在 B1 执行统一 `check`，报告绑定两侧实际入口及等价的九项逻辑能力，不能只按新旧命令名比较。运行记录还会把两端实际源码身份绑定到来源证据，并在开始采样前复核一次。报告会明确记录实际适配提交及来源。受控补丁保存在主分支，因此临时适配分支删除后仍可重建；测量入口始终是 `cargo xtask check perf paired`。

测量产物写入 `.local-tests/devex/<日期>/<run-id>/`，包含源码与环境指纹、逐次样本及 P50/P95/P99；编译缓存 suite 同时保存前后统计。进程树峰值内存区分 Windows Job 提交内存与 Linux cgroup 计费内存，后者需显式设置有写入权限的 RYFRAME_DEVEX_CGROUP_ROOT；Extended CI 显式运行独立 cgroup 的孙进程用例，保存降权运行、测量与精确清理证据，缺少父级 memory controller 时失败。采集不可用保留原因，不记为零，不同口径拒绝比较。runner 校验源码、工具、缓存语义与样本完整性，输入变化、失败或证据缺失均阻止达标结论。Windows 深层工作树的临时 Cargo target 使用 `.local-tests/d/<摘要>` 避免长路径，测量报告仍保存在 DevEx 目录。`cargo-dev-save` 使用真实 watcher、只读迁移验证和探活，不自动升级数据库。日常 `CI` 使用 6 个业务任务加 `Required` 汇总；`Extended CI` 定期或由 tag、手动及部署/全栈脚本变更触发，验收容器、SBOM、镜像与真实浏览器。全栈准备仅在本 Job 创建控制库、共享库与两个独立库，写入不含密码的临时目标配置，完成初始化与 verify 后启动 API、external Worker；开发服务器与生产构建均连接同一隔离环境。
## 真实运行性能

运行时测量使用同一 DevEx 入口：`runtime-homepage --variant default` 分别测量 `cold` 和 `warm` 浏览器缓存；`runtime-api`、`runtime-jobs`、`runtime-tenants` 使用 `--variant 10|50|100 --cache warm`。后者不宣称清空服务器缓存。固定回归使用 10 并发，50、100 并发用于容量探索。每个 workflow 与 `contract.homepage` 必须显式选择 `identity_pool`：`contract.identity_pools` 固定 `roles_sha256`、`permissions_sha256`、`client_address_model: 'fixed-benchmark-per-user'`、有序 `slots`（`tenant_slot`、`user_slot`、`client_address`）及各选中数量对应的租户分布 `selections`；`bindings.identity_pools` 按相同顺序绑定 `tenant_id`、`username`、`password_env`。角色权限摘要应在准备阶段核对真实认证上下文后登记。实际前 N 个用户、逻辑用户槽位和固定测试地址必须独立，逻辑租户与真实租户一一对应，分布必须匹配 contract，尾部身份不能补齐前 N 的缺口。多租户及导入场景均匀覆盖十个普通租户，10 并发导入每租户一个用户；调度使用对应数量的 system 普通用户。单租户导入活动任务上限保持不变，50/100 探索中的真实限制与失败照常记录。`identity-selection.json` 保存脱敏选择、分布与摘要，不把账户名或密码写入选择收据。用 `node scripts/devex_prepare_identities.mjs plan --environment <manifest> --output <plan> --write` 审查固定 200 个普通测量账号，再用 `apply|verify --plan <plan> --state-dir <后端独立忽略目录> --write` 创建或核验。工具只接受预先登记的独立性能环境，保留真实权限、模板与固定消息 audience 收据；未知写入结果标为 `needs-reconciliation`，不自动重放。

开发数据复制、性能 seed、来源捕获和中断续作的操作边界统一写在[数据文档](data.md#开发数据复制与性能-seed)。这些流程只接受已登记的隔离资源；默认状态与复核入口保持只读，任何写入、恢复或续跑都要求显式参数及原账本前后像。

在被测后端的忽略目录 `.local-tests/devex/runtime.json` 提供 `schema_version: 1`、`contract` 和 `bindings`。`contract` 包含固定数据集、硬件/存储/网络和服务参数的 SHA-256、每场景 `cycles`、`timeout_ms` 与 `workloads`；两个对照环境必须使用相同 contract。`bindings` 包含明确的隔离 `scope_id`、`source_fingerprints`（被测后端、被测产品前端和共享 runner 前端三项完整 dirty 指纹）、API/前端地址、API 与 Worker 的 `metrics_urls`、MySQL exporter 的 `database_metrics_urls` 以及独立身份池。数据库连接指标是整实例口径，共享实例及采集连接必须如实记录，同一实例不能按 schema 重复登记。密码仅通过身份的 `password_env` 指定环境变量名，不能写入 JSON。API/Worker 指标鉴权使用可选 `metrics_token_env`，该 token 只发给服务指标端点，不发给数据库 exporter。`bindings.provenance` 必须登记绝对 `python`、`backend_build: {path, sha256}`、`runtime: {path, sha256}`、`processes: {api, worker}`（各进程收据 SHA）、`frontend_build_sha256` 与绝对 `environment_document`；最后一项是脱敏 UTF-8 环境说明，实际文件 SHA 必须等于 `contract.environment_sha256`，说明硬件、数据规模、存储、网络与服务参数的实测或登记依据，收据本身不能证明这些事实。后端使用 `cargo xtask check recovery runtime build --source-backend <后端绝对工作树> --expected-head <后端完整 SHA> --source-frontend <前端绝对工作树> --expected-frontend-head <前端完整 SHA> --output <后端工作树内的忽略目录/build.json> --write --frontend-dir <当前工具前端>` 生成 Cargo 产物收据，运行绑定使用同一工作树的全栈 `runtime.json`、`api.json` 与 `worker.json`，并继承启动时同一 APP 环境；同一入口调用当前干净工具构建历史前端，在目标前端写入并严格核验 `dist/.vite/restore-build.json`，已有有效收据只核验和复用，不覆盖。当前 v2 收据把 product、tools、full 三类来源分开记录，并绑定实际构建命令与工具链；只改工具时可在显式来源审计中复用产品输入未变的产物，构建参数、产品输入或前端 production 环境文件变化时必须重建。性能入口允许精确 dirty 快照，正式恢复仍要求 clean SHA；每个样本前后验证源码、收据、二进制、进程创建身份、实际监听及站点资源字节，并把产品前端与 runner 前端保存为两个独立来源。失败写入 `provenance-before.json` / `provenance-after.json` 并拒绝通过。

API 场景固定为 list、filter、page、login-refresh、write；任务场景为 export、import、message、schedule；多租户场景为 list、write、export。各场景的 `steps` 使用当前 OpenAPI 的 `operation`、`path`、`query`、`body` 或 `multipart`；`save` 用 JSON Pointer 保存字段，`${unique}`、`${tenant_id}`、`${cycle}` 等变量逐周期展开，`expect` 核对响应，轮询只允许 GET。导入在 `contract.import_samples` 固定真实模板 `template_sha256` 和 `hash(importSampleModel)` 的 `model_sha256`，用 `bindings.import_samples` 的绝对 `python`、`template` 路径绑定准备器与模板；multipart 使用 `sample: '${worker}:${cycle}'`，与固定上传的 `path_env` 恰选其一。每次先离线生成独立命名空间的全有效 XLSX，每文件一行、单次最多 10000 文件，按清单校验 SHA 与目录范围，不复用错误行 fixture。`import-preparation.json` 保存模板/模型/文件摘要及准备耗时；准备和前后来源校验不计入业务请求和服务资源观察，外层命令总耗时及进程树峰值仍包含这些工作。后台场景通过 `job: {kind, id, job_type}` 关联导出/导入任务、消息或调度 execution ID，不能用计划 ID 代替。`bindings.job_timings` 提供绝对 `python`、`mysql_client` 路径、`server_uuid` 及 `control`（host、port、database、username、password_env、tls_mode）；采集器只读精确隔离库并校验 scope、租户和关联类型。`job-attempts.jsonl` 保存完整领取时间事实，失败、重试和未知结果如实保留，不能用 `updated_at` 替代或在证据完成前删除任务记录。

指标端点必须提供 API/Worker 的 `ryframe_process_cpu_seconds_total`、`ryframe_process_resident_memory_bytes`、`ryframe_process_start_time_seconds`；MySQL 必须提供唯一非负整数 `mysql_global_status_threads_connected`，且 `mysql_up=1`、`mysql_exporter_collector_success{collector="collect.global_status"}=1`。缺失、重复、失败均拒绝采集，连接数保持整实例口径。每个服务进程单独核对采集与重启，采集错误、业务或会话失败、场景漂移均会阻止通过。驱动记录真实导航、LCP、关键内容可用时间、HTTP 请求数、业务周期 P50/P95/P99、周期吞吐与失败率、独立会话失败数、排队/执行时间和峰值内存；一个周期可含多个请求，首页多个指标共享的失败周期只累计一次。JSON 响应限制 16 MiB，下载按流计数。首页使用真实登录和 Chrome/Playwright，不使用浏览器 fixture。`contract.homepage.visitor_interval_ms` 显式设置访客启动间隔，每个周期使用已登记的独立身份和固定客户端地址，只接受本机 loopback API 与前端。`homepage-cycles.jsonl` 保留成功及失败周期的阶段和诊断分类；其他场景在 `cycles.jsonl` 和 `session-events.jsonl` 保留业务周期、登录与退出结果。登录失败导致未完成的预定周期计为失败，退出失败单列；失败耗时不混入成功分位数，失败记录仍完整报告。

`paired` 要求协调器、基线和候选各有独立前后端工作树，并以 ABBA 顺序交错采样。运行时两侧分别绑定自己的产品前端源码和生产产物，但都从协调器当前受信且精确绑定的前端加载 Node 依赖、Playwright 和登录预算实现；runner 前端的 SHA、dirty 状态、完整指纹与执行来源在样本前后复核，不能把新 helper 写入 B0 产品树或把两侧测试工具差异算成产品性能。普通对照每侧至少 6 个有效样本，组成 3 组 ABBA；编译链与 AWS-LC 关键对照每侧至少 20 个。现有预算继续生效，保留场景 P95 回退超过 10% 失败。新增场景先形成基线，不与旧统计范围混比。`contract.pacing` 固定 `request_interval_ms`、按 operation ID 声明的 `operation_intervals_ms`、`sample_prepare_wait_ms` 与 `authority_sha256`；限流事实通过同一 APP 环境运行 `python scripts/full_stack_rate_limit_config.py --authority` 取得，以 `scripts/devex/config.mjs` 的 `hash` 计算规范化 JSON 摘要。`bindings.pacing` 显式提供绝对 `python` 与共享 `login_budget_state` 路径。业务前校验全局、启用的用户限制、接口规则及登录 principal/IP 限制，间隔须满足实际 authority，示例参数不能替代当前配置。每次调用（含预热和 ABBA 两侧每个样本）都先等待固定准备窗口，`sample-preparation.json` 记录起止时间、原因及实际等待；`pacing-authority.json` 和 `pacing-summary.json` 保留配置校验与等待摘要。程序化 HTTP 的固定节奏涵盖嵌套 CSRF，并跨场景保留同一客户时钟；业务周期分位数和吞吐包含业务阶段固定等待，准备窗口不计入周期或服务资源观察，外层命令耗时和进程树内存包含全部准备。真实首页仅使用样本准备窗口，不拦截页面请求来改变 LCP。身份准备可复用 `createPacing` 的 `createPreparationControls`，登录预算沿用 runner 前端共享账本，失败预约不退款。所有 429 都保留为失败，不重试、不换地址、不调整产品限制。运行报告保存在忽略目录，不能把工具完成执行等同于实际环境达到性能目标。

## 资源门禁

`cargo xtask check ci resource-gate --frontend-dir ../ryframe-vue3` 直接从 CI 的 base/head SHA 计算资源变化、关系闭包和 ownership，不接收调用方拼接的资源名。缺少合法 base、变更面过大、删除或重命名无法归属，以及 Cargo、toolchain、模板、CI 或架构策略变化时会自动执行完整门禁。

需要重新证明定向门禁与完整门禁一致时，通过同一 CI 分组运行回放，不直接调用内部 Python 程序：

```powershell
cargo xtask check ci resource-gate replay --manifest <案例清单> --work-dir <后端 .local-tests/resource-gate-replay 下的新目录> --report <报告文件> [--activation-gate] --frontend-dir <前端目录>
```

xtask 固定当前后端和显式前端仓库，清单至少覆盖新增、字段、权限、关系、SQL、重命名与删除，并同时包含成功、失败、定向和完整回退案例。`--activation-gate` 还要求零分歧、健康缓存、足量定向成功案例和当前耗时预算；缺少任一证据时继续使用完整回退。

定向模式的 Clippy 只检查受影响库和二进制，测试阶段只运行资源契约测试；MySQL、Redis、对象存储和 OTLP 的真实协议测试统一由 integration 门禁执行，避免在每次资源改动中重复编译重量级协议 harness。

定向模式当前默认关闭；只有经过独立后端与前端提交回放、证明定向结果与完整门禁一致后，CI 才能使用受控激活标记。激活证据缺失、过期或无法验证时继续完整回退，不会把完整回退误报为定向通过。CI 只在编译面最大的 Rust Gate 与 Windows Smoke 使用 sccache，并通过 GitHub Actions 缓存持久化限定大小的本地编译缓存；其他任务不安装无收益的编译包装器。缓存只包含编译结果和 Cargo 依赖，不保存整个 Cargo target；缓存配置或统计异常不得替代真实门禁结果。

## 常见问题

- 启动提示迁移不一致：运行 `cargo xtask data migrate status` 与 `cargo xtask data migrate verify`，确认后再执行 `cargo xtask data migrate up`；Worker 未消费任务时，确认 API/Worker 使用 `APP_JOBS_MODE=external`，检查健康端口、lease 和数据库连接。
- 前端请求与后端不一致：重新运行 `cargo xtask generate api --write` 和消费者检查；Redis 或对象存储不可用时，检查 `scope_id`、连接模式、TLS、ownership marker 与服务端口，详见[运维指南](operations.md)。
