# 架构与扩展位置

[English](architecture.en.md)

MQTT、邮件和乐观锁等插件属于后续路线：优先采用按需 Rust crate 与编译期装配，未引入、未启用时不增加服务要求。MQTT、邮件按集成能力设计，乐观锁按持久化扩展设计；届时同时处理依赖、配置、生命周期、权限菜单、租户隔离、能力目录及套餐同步。当前不实现插件系统，不添加空接口、动态加载开关或占位依赖。

本文帮助业务开发者理解一次请求如何经过 RyFrame，并选择合适的扩展位置。

## 运行结构

```text diagram
浏览器 / 客户端
       │ HTTP / WebSocket
       ▼
ryframe-api（路由、DTO、认证提取、OpenAPI）
       │
       ▼
ryframe-application（业务用例、事务、状态机、端口）
       │
       ├── ryframe-db / ryframe-tenant-db（控制库与租户库）
       └── ryframe-adapters（Redis、对象存储、表格、限流、遥测）
       │
       ▼
ryframe（API、Worker、迁移与依赖装配）
```

HTTP 层把请求解析为明确的 DTO，再调用 application 用例。用例通过端口访问数据库、Redis 和对象存储，因此业务流程可以在不依赖具体连接实现的情况下测试。API 与 Worker 使用同一组应用服务。组合根通过 `APP_ENV=dev|test|prod` 选择必须存在的 `config/app.<environment>.toml`，与公共 `config/app.toml` 合并；密码、JWT、监控、Redis 和存储凭据使用明文字段，初始管理员密码来自 `[bootstrap] admin_password`。根字段 `snowflake_worker_id` 在同一部署的各实例间必须不同。应用 `--config-dir <directory>` 默认 `config`，相对当前工作目录解析；生成器的 CLI、Builder `.configuration_directory(path)` 及可选 `[generator] config_dir` 相对后端 Workspace 根目录解析。生成器优先级为显式 CLI/Builder、本机 `.local-tests/tools.toml`、公共 `config/tools.toml`、`workspace/config`；工具文件不接受环境变量覆盖。本地化通过 TOML 的 `app.locales_dir` 指定，默认 `locales`，普通应用相对进程当前工作目录解析，不读取 `APP_LOCALES_DIR`；生产加载资源失败时严格拒绝启动，开发环境允许内嵌回退。开发监督器已实现按代次保存完整私有文件配置、配置保存后切换及失败回滚；端口与实例编号变化复用既有二进制，回滚使用旧代次的配置、端口和编号。停止状态不确定时保留现场并拒绝启动第二个 Worker。真实服务整链验收仍待完成，不能用定向测试代替，使用方法见开发指南。部署四角色模板与私有配置目录已集成并通过资产结构及 CI 工具测试；真实容器、镜像和整链仍未验收，局部检查不代表整体门禁。

## 模块定位

| 模块 | 开发时用于 |
|---|---|
| `ryframe-kernel` | 通用 ID、Snowflake、分页、错误和值对象 |
| `ryframe-config` | 配置结构、明文 TOML 文件合并、校验及工具共用版本阶段判定 |
| `ryframe-auth` | 密码、JWT 与 RBAC 决策 |
| `ryframe-application` | 业务用例、事务、状态机和出站端口 |
| `ryframe-db` | 控制库查询、写入与迁移 |
| `ryframe-tenant-db` | 租户目标路由、查询、写入与迁移 |
| `ryframe-adapters` | Redis、对象存储、表格、限流、本地化和遥测 |
| `ryframe-api` | Axum 路由、DTO、OpenAPI、extractor 和传输中间件 |
| `ryframe` | API、Worker、迁移和重建的启动装配 |
| `ryframe-sdk` | 使用者业务 crate 的稳定资源、路由、持久化与迁移扩展接口 |
| `ryframe-generator` | 标准资源的离线生成；默认构建不包含数据库驱动 |

## 进程编译面

根组合 crate 默认只编译 API 与 Swagger UI。独立进程必须关闭默认 feature，并只选择对应入口，避免把 HTTP、Redis、图片处理和遥测带入迁移工具：

| 进程 | feature |
|---|---|
| API | `bin-api`，按需附加 `runtime-swagger-ui` |
| Worker | `bin-worker` |
| 控制库迁移 | `bin-migrate` |
| 租户数据维护 | `bin-tenant-data` |
| 非生产重建 | `bin-reset` |

`ryframe-adapters` 默认不启用高成本能力；API 显式组合 `image-processing`、`monitoring`、`otel`、`redis-api` 与 `spreadsheet`，Worker 使用 `monitoring`、`otel`、`redis-client` 与 `spreadsheet`。验证码只启用 PNG codec，通用图片处理 codec 仅随 `image-processing` 编译。生成 fake 只在 `ryframe-application/test-support` 下可用，不进入产品进程。

Worker 的健康状态模型与数据库监控端口位于 application，不依赖 `ryframe-api`；HTTP 健康端点只在 Worker 组合根装配。所有二进制在创建 MySQL、Redis、HTTP、对象存储或 OTLP 客户端前统一安装 AWS-LC provider；遇到已安装的不同 provider 时拒绝启动。Security Audit 解析真实 Cargo feature tree：API（含 Swagger UI）与 Worker 要求 JWT、HTTP、Rustls 和 SQLx 链路启用 AWS-LC，并拒绝实际 `ring` package/provider；Worker 额外禁止依赖 `ryframe-api`。迁移进程只保留数据库所需的 AWS-LC TLS 链路，禁止 API、Adapter、Web、Redis、图片处理、遥测和 HTTP client 依赖。生成器默认运行图禁止数据库与网络 runtime，unique package closure 上限为 90；schema import 依赖仅由对应 feature 引入。`aws-lc-rs` 自身的 `ring-io`、`ring-sig-verify` 兼容 feature 不属于 ring provider。统一 provider 只描述项目选择的密码学实现，不代表项目自动获得 FIPS 认证。OTLP 的 HTTP protobuf 出站链路只编译 trace、Tokio runtime 与 reqwest client，不包含 logs、metrics 或 blocking client。生成器与 xtask 的稳定版阶段判断共用 `ryframe-config` 的 tools 能力，只读取 Workspace 清单、Git tag 和历史提交；迁移历史冻结工具不调用 Python 或数据库。

遥测仅使用文件配置：请求头、资源属性、span 限制及批处理参数均由合并后的配置生效，`OTEL_*` 环境变量不生效。字段与默认值以 [默认配置](../config/app.toml) 和 `TelemetryConfig`、`TelemetrySpanLimits` 类型为准。资源中的 SDK 元数据、服务名称、版本及构建提交身份由框架确定，不能通过自定义资源属性覆盖。文件直接保存显式配置值；`Debug` 完整隐藏端点、请求头和资源属性值，校验错误不回显凭据。采样、AWS-LC TLS、导出失败指标及优雅关闭行为继续保留；真实 OTLP HTTPS 验收仍须单独执行，不能由配置检查替代。

本地 `opentelemetry_sdk` 与 `opentelemetry-otlp` 补丁提供显式不读取环境变量的构建入口，使遥测配置不受 SDK 或导出器的环境默认值覆盖，同时保留第三方原有默认入口；两者保持 `0.33.0`。SDK crate 发布包未附许可证原件，本地 SDK 与 OTLP 的 Apache-2.0 许可证均采用经同一上游提交核对的 `opentelemetry-proto 0.33.0` 许可证来源，文件摘要与来源登记见 [供应链策略](../tools/policies/supply_chain_policy.json)。

`ryframe-application::system` 按业务分为四个入口：

- `identity`：用户、角色、权限、部门、档案、导入、验证码和 WebSocket ticket；
- `platform`：租户、产品和授权诊断；
- `content`：配置、字典、公告、文件、选项和标准内容资源；
- `operations`：消息、导出、审计日志、登录日志、在线用户、监控、备份恢复验证和保留策略。

查找现有能力时，先从对应业务域的公开服务开始，再进入具体用例。

## 选择开发方式

跨仓 OpenAPI 派生文件以开发者前端仓库的 `scripts/api-artifact-manifest.json` 为清单事实源，前端生成器与后端契约事务共同读取。版本必须为 1，`files` 是非空、无重复的安全 `src/` 相对路径；清单、渲染器及其他输入均参与事务前后快照校验，变化时拒绝安装候选结果。

RyFrame 官方源码不内置业务示例。开发者用 `cargo generate -- new-business <business-package-name> --write` 在 `crates/` 下创建业务 crate，例如 `order-business`、`inventory-business`，通过 `ryframe-sdk` 使用框架扩展接口。省略 `--write` 预览；脚手架原子写入工作区成员、组合根依赖和唯一注册表。一个业务 crate 默认同时包含模型、用例、Repository、Handler、路由和迁移；业务复杂后再按实际边界拆分。

业务模块通过普通 Cargo 依赖显式接入。`crates/ryframe` 是唯一组合根，`business_modules()` 是唯一注册表；API、Worker 和迁移程序共用该列表。该结构不会使用运行时目录扫描、自动链接注册或构建脚本改写源码，Cargo 依赖图和模块依赖图都可以静态检查无环。

标准 CRUD 从手写 Rust `ResourceModel` 生成到业务 crate 自己的 `src/generated/` 与 `migrations/`。目录按 `entities`、`dto`、`repositories`、`services`、`handlers`、`openapi` 分类，便于阅读；`resources/` 和 `extensions/` 始终由开发者维护。默认 catalog 编译只依赖轻量 SDK，不引入数据库或网络 runtime。

框架资源的 `render_resources` 只离线生成后端资产；需要两仓资产时使用 `render_resources_with_frontend` 显式指定前端模板来源，`cargo xtask generate resource` 沿用跨仓检查与写入，`--explain` 无需前端。后端单独计划与写入不读取或校验前端路径，完整保留旧前端 ownership 条目；显式跨仓操作重新核验全部请求范围的受管文件。命名生成绑定实际渲染来源并只注册已受管资源和本次资源，不把未请求的前端输出当作删除。

需要事务编排、外部连接、异步任务或特殊状态机时，使用自定义用例：

1. 在业务 crate 的 `extensions/` 定义请求、结果和业务流程。
2. 需要外部能力时定义端口并在该业务 crate 实现，框架能力通过 `ryframe-sdk` 使用。
3. 在 `module()` 中装配路由、OpenAPI 和迁移；后台执行由 Worker 从同一模块注册表取得能力。
4. 同步前端契约并完成联调。

标准资源也可以保留一个强类型扩展，例如 Post 导出或 Notice 消息发布；其余常规 CRUD 继续由资源清单生成。

## 数据与事务

控制库保存身份、授权、租户目录、placement 和平台任务；租户业务数据通过控制库中的 placement 路由进入 shared-control、共享业务目标或独立租户目标。涉及多步写入时，由 application 用例开启并提交事务，同一流程中的 Repository 调用接收同一事务上下文。`database.sources` 与各租户目标使用独立连接池和事务；框架不自动分库，也不伪造控制库、来源库和目标库之间的分布式事务。

列表展示可按场景选择 eventual consistency；权限校验、任务领取、下载和状态转换使用 strong consistency。租户切换和后台任务应继续传递明确的租户与作用域信息。

## 访问控制与契约

路由使用 `Public`、`Authenticated`、`Permission` 或 `Capability` 访问策略。框架菜单、权限、页面键和 capability 来自 `catalog/access.toml`；标准业务由 Rust 模型生成模块内 `access.toml`，初始化、SQL 导出和 OpenAPI 装配共用解析结果。业务路由在 API 层关联对应策略；组合根一次安装系统租户专用业务边界，租户开通、权限管理及会话投影使用同一边界。

接口的请求与响应进入 OpenAPI 快照，前端从快照生成 operation descriptor。接口变更后的同步步骤见[开发指南](development.md)。

## 外部备份与恢复验证

历史进程停止证明由 Rust 统一执行，其他调用方只传递完整创建身份并消费严格的小结果。Windows 的 CIM 查询通过仅工具 crate 启用的 `wmi` 安全 API，在受管原生子进程内完成；第三方 COM 实现内部不属于 RyFrame 自维护 Rust。CIM 查询结果与停止结论分开，查询失败不能代替停止证明，具体边界和排障方式见[运维说明](operations.md#采集和登记)。

seed 的 source-register 由统一 Rust 阶段控制器直接执行。它实际持有目标 Generation 和复制账本 guard，核验完整初始化、复制、seed close 及生产者历史，按固定顺序发布六项本地证据，并在写入最终来源登记前完成两轮来源锚点、请求和 generation 比较。正常收尾核验并释放本次持有的锁；失败保留局部证据，异常残留锁须按实际状态核对，不能删除后重放。Windows 原生 CIM 查询失败会拒绝登记，不回退 PowerShell/PDH。阶段成功返回包含内层登记结果的统一外壳；调用方法和结果消费见[运维说明](operations.md#参考环境的外部验收驱动)。该本地登记不授予远端写入权限，也不能替代完整端到端恢复验收。

外部运维工具负责生成一致的数据库与对象备份。application 的备份用例拥有登记、恢复状态机和事务；控制库 Repository 保存清单及演练记录，租户库实现目标与数据校验，adapters 实现文件摘要、对象摘要和就绪探测。维护 CLI 负责读取显式配置、装配端口并打印结构化结果。API 后台采集器只读取汇总状态，向 Prometheus 输出固定维度指标。

恢复依次经历开始、数据验证、业务与运行验证；失败为终态。目标必须具有独立 scope 和 ownership，数据校验比较明确登记的物理数据库、schema、租户关系、表摘要与对象摘要。业务证明绑定演练记录和精确前后端源码，只有数据校验、完整浏览器场景、API/Worker 就绪以及恢复时间目标都满足时才登记成功。具体操作见[运维指南](operations.md#备份失败或过期)。

### 生成恢复业务证明

使用 `cargo xtask check recovery inputs bindings --reference-plan <side-reference-plan> --target-plan <side-target-plan> --backup-receipt <same-backup.json> --record <data-verified.json>` 只读预览数据绑定；加 `--output <new-bindings-file> --write` 才原子发布。该入口从已核验记录和原备份清单推导唯一的 `record`、`manifest`，不接受人工摘抄或重组。前后端必须是清单绑定 SHA 的干净源码，使用 `cargo xtask check recovery runtime build --source-backend <absolute-backend-worktree> --expected-head <full-backend-sha> --source-frontend <absolute-frontend-worktree> --expected-frontend-head <full-frontend-sha> --output <backend-worktree/.local-tests/restore/build.json> --write --frontend-dir <current-tools-frontend>` 统一生成并核验实际 API、Worker 和前端生产构建收据。历史源码不需要提供当前 CLI；协调器使用当前干净工具调用目标工作树自己的 Vite，并在目标前端写入 `.vite/restore-build.json`。已有前端收据会经过相同的严格核验后复用，绝不覆盖或补造构建事实。当前后端构建收据使用 v2、前端构建收据使用 v3，分开绑定按产物角色计算的产品输入、验收工具输入、完整源码清单、实际工具链和有效构建参数。前端从收据保存的 `configuration.environment` 选择 `app.dev.toml`、`app.test.toml` 或 `app.prod.toml`，按公共 `app.toml`（可缺省）、所选环境文件（必需）的读取顺序绑定路径和摘要；同时绑定 `config/tools.json` 与 `.local-tests/frontend-tools.json` 的实际来源及优先级、受管 `.local-tests/build/source.json` 身份和完整 `dist` 清单。核验不依赖当前 shell 的 `APP_ENV` 或旧 `VITE_*`；真实构建要求合并后的 `api_origin` 为空，正式来源身份必须等于声明的精确干净 HEAD，且与 `dist/build-identity.json` 一致。旧格式仅保留为历史文件，不能用于当前恢复。

`runtime start` 必须同时传入 runtime registration、完整 target plan、双端来源、构建收据和数据绑定；Rust 按 API、Worker、前端顺序启动服务。API 和 Worker 读取运行登记绑定的私有角色配置，启动和核验阶段重验配置来源和文件。正式恢复登记保留原 fresh 文件及公共配置字节，在受管 API 的 `app.test.toml` 副本中显式设置 `jobs.scheduler_enabled = true`，Worker 从该副本仅修改为独立实例编号；其他有效配置语义保持不变。派生前按包含公共配置继承值的有效 TOML 校验 `jobs` 为表、已有 `scheduler_enabled` 为布尔值，缺省时在副本中补齐，非法类型直接拒绝。后续核验继续严格比较完整受管文件字节和目录身份。来源登记仍要求种子系统计划保持 disabled；此处只启用角色调度能力。它在启动前保存角色 operation intent，使用 Windows Job Object 或 Linux 独立 session、进程组与 pidfd 登记并回收整树，再核对各角色的就绪内容和监听归属。首次创建运行目录前还会在目录外写入唯一 creation intent，因此控制器在目录创建、状态写入或进程收据发布的任一切点退出后，都只能通过绑定 owner 和 generation 的 `runtime recover --write` 接管。 Rust 在同一内核互斥区间内核对原 owner 的创建身份，保留回收收据，再接管原代次。首代状态缺失时只接受对应的启动 owner 或能沿回收收据追溯该 owner 的接续控制器。有原创建意图时，从该意图和三端口闭口证据恢复生命周期，随后复用正式停止路径。接续失败时保留已接替的 owner 和原收据。每个 generation 只允许当前 operation ID 对应的日志、进程、完整树、成员、控制、结果、完成和 launch 收据；未知文件、目录或链接会阻断继续操作并保留现场。`runtime status` 由 Rust 在同一控制锁内只读核验登记、首代创建意图、代次收据及三端进程和端口，派生未启动、中断、运行、降级、退出或停止状态。它不启动或停止进程，不写生命周期或角色收据。`runtime stop --generation <N> --write` 由 Rust 在同一控制锁内核验指定代次，按前端、Worker、API 顺序回收原监督树，验证原成员关闭收据及三端口闭口。已停止的代次在重新核对后返回 `already_stopped`。角色回收失败时仍尝试其余角色，并保留代次状态和原收据。 正式恢复的 Rust 前端服务读取当次启动代次登记，提供已核验的 B0 或当前前端产物。同源请求 `/api/v1` 和其子路径转发到当次登记的 IPv4 回环 API，`/api/v1/ws` 支持 WebSocket 升级。启动时核对 API 的创建身份、监督树和监听端口，每次建立上游连接时核对实际连接归属。API 和前端使用不同的回环端口。
三端就绪后，用 `cargo xtask check recovery runtime bind --source-backend <execution-backend-worktree> --source-frontend <frontend-worktree> --build-receipt <build.json> --launch-receipt <generation-directory/runtime-launch.json> --bindings <bindings-file> --output .local-tests/restore/runtime.json --write` 签发 v3 运行收据。普通候选的产品与执行后端必须是同一提交；B0 另外显式传入 `--adapter-contract legacy-stable-readiness-b0-v1 --product-backend <815c5eaf-product-worktree>`，执行来源必须是已登记的 `c9d88b76` 适配提交。运行收据分别保存备份来源、后端产品来源、后端执行来源、适配合同、前端来源以及三进程 generation，不能用备份清单的源码 SHA 冒充运行二进制来源。该入口核对源码、二进制、进程创建身份、完整树、探针监听端口及实际返回的前端文件，配置目录正确或 HTTP 200 均不能单独充当来源证明。 前端 v3 收据、选中 APP_ENV 对应的文件配置、受管身份与完整 dist 由同一 Rust 权威只读校验；恢复适配只传路径与无正文事实，发布/使用窗口末端重新核对，不能将 IPC 快照视为持续文件保护。 前端站点由本次登记的 Rust 协调程序启动，只监听 `127.0.0.1`。它在监听前核对原构建收据的 SHA-256、前端提交及完整 `dist`，服务期间只提供本次核验的文件字节。运行权威复核该可执行文件的路径、字节数和摘要，以及已记录的前端启动参数。 `cargo xtask check recovery runtime verify` 使用 Rust 只读核验单侧运行。命令从标准输入接收权威恢复上下文，按显式路径复核数据绑定、当前构建、启动与进程收据、监听归属、三端响应及全部生产前端文件，成功时返回同一运行收据摘要和详细 JSON 结果。 `cargo xtask check recovery runtime bind` 在原恢复登记锁内复核同一登记、参考计划、目标计划和运行代次，再在线核验该代次的三端与构建产物。成功后独占发布 v3 收据并返回 `{"output": …}`。发布后复核失败时保留已创建的收据供核对。

在隔离验收配置文件中成组登记绑定文件、运行收据、严格目标计划、当前干净协调器源码路径、当前干净前端测试 runner 完整 SHA 和当前干净后端核验器完整 SHA，并绑定新 scope 与已启动的生产站点。字段和文件选择参数以配置事实源及 CLI help 为准，接线仍待整合核对；原数据身份从严格目标计划绑定的备份、同代 STOP/START 与 source-runtime 递归推导，再运行 `corepack pnpm check --stage browser --real --server preview`。目标计划和运行收据独立绑定实际产品前后端，因此 B0 产品前端可以与当前测试 runner 不同。首次登录前，测试 runner 通过已核验后端的原生 `cargo xtask __source-evidence` 执行只读数据预检，完整重验目标计划、来源代次和血缘，并将运行收据的 launch 规范路径与摘要、lifecycle、构建、端点和三角色进程文件交叉绑定后，取得内存中的唯一文件绑定；预检不访问服务、不创建控制锁或 Python 字节码；预检失败不得进行登录、注销或业务请求，不自行抽查少量收据字段替代完整核验。测试开始及结束均复核来源；运行期间替换进程、修改源码或产物都会拒绝生成成功证明。各文件只保存摘要和资源归属，不包含密钥。 恢复后的既有数据核验由测试 runner 调用 Rust 核验入口。入口复核同一份数据权威、目标运行代次和原数据血缘，在登记互斥下登录、逐租户核对完整岗位样本及对象字节，然后注销。证据单独记录核验范围和实际计数。核验失败时保留当次证据和运行状态，先核对现场再决定如何接续。

测试覆盖全部恢复核心场景并通过控制台、网络和 axe 断言后，结果写入 runner 的 `.local-tests/playwright-real/restore-<drill-id>.json`，测试明细和运行收据副本使用同一前缀相邻保存。证明明确记录备份来源、后端产品与执行来源、适配合同、产品前端、测试 runner 和当前后端核验器 SHA；测试明细以 `sources.coordinator`、`sources.verifier` 和 `sources.runner` 分别绑定协调证据、核验工具和测试源码的根及精确 SHA，并绑定演练、目标计划、运行收据、产品来源与测试时间，并保存唯一 STOP source-generation 与派生 dataset-lineage 的完整文件描述；证明保存这两份文件的 SHA-256。已有结果不会覆盖；有失败、跳过、重试、缺场景、源码不匹配、明细不一致或数据验证尚未完成时不生成成功证明。最终 `restore verify` 必须同时收到三份相邻证据、原目标计划、协调根与 SHA、runner 根目录；维护 CLI 会从登记库构造权威上下文，由当前核验工具从原协调根内 runtime launch 绑定的 registration 读取本侧参考计划，重新核验目标计划、运行中三进程、完整构建来源、runner 以及全部摘要，再登记成功。数据血缘必须与同一备份的 STOP、START、当前 source-runtime 全部一致；另一份独立 lineage 或另一侧参考计划不能替代此链。这一步仍会重新检查服务就绪和恢复时间，浏览器通过不等于最终恢复登记成功。最终核验使用统一 data 入口在编译时绑定核验源码 SHA，并在同一活构建锁内登记独立协调来源与核验器的路径、字节数和摘要；直接运行旧维护二进制会失败。浏览器测试在打开首个会话前调用 Rust 只读入口 `restore-dataset-authority`，获取绑定的运行收据、目标计划、STOP 与 dataset-lineage，以及本侧隔离范围和端点。最终 `restore verify` 以版本 2 权威上下文将数据库记录、正式备份清单和工具绑定交给同一原生工具的 `restore-business-proof`。维护 CLI 和原生工具在核验前后复核证据、程序身份和源码。

### 独立 B1 参考计划

用 `cargo xtask check recovery inputs reference --independent-b1 --arm-input <independent-run/result.json> --side candidate --id <backup-id> --work-dir <new-b1-reference-work-dir>` 预览独立 B1 参考计划。追加 `--output <new-b1-reference-plan.json> --write` 可发布计划，输出文件和工作目录须使用不同位置。原运行须已收尾，且请求中的复制目录仍须不存在。工具重新核对独立目标和原共享导出，允许来源账本在原记录逐项保持一致的基础上追加已核验的终态阶段。计划绑定独立 ARM 结果及原共享导出，写入范围限于指定的新计划文件。登记结果保持 `restore_qualified=false`，实际复制须先独占声明请求绑定的复制目录。

取得该独立目标的 `verify.json` 后，用 `cargo xtask check recovery inputs product --reference-plan <b1-reference-plan.json> --backup-receipt <same-backup.json> --comparison-sources <comparison-sources.json> --arm-input <independent-run/result.json> --fresh-target-verify <b1-observation/verify.json> --side candidate --id <restore-id> --fault-at <utc-time>` 预览产品计划。追加 `--output <new-b1-product-plan.json> --write` 可发布。工具按原配置重新生成并核对参考计划，要求原独立 ARM、正式备份和双版本来源绑定同一共享导出。产品前端提交取自原比较清单的 B1，物理目标和探针取自独立登记。复制目录继续保持未创建。

用 `cargo xtask check recovery plan --plan <b1-reference-plan.json> --backup-receipt <same-backup.json> --comparison-sources <comparison-sources.json> --arm-input <independent-run/result.json> --fresh-target-verify <b1-observation/verify.json> --product-plan <b1-product-plan.json>` 预览完整目标计划。追加 `--output <new-b1-target-plan.json> --write` 可独占发布。用 `cargo xtask check recovery plan --plan <b1-reference-plan.json> --target-plan <b1-target-plan.json>` 回读，工具按原五件输入重新投影并核对完整参考计划封装、同一备份、双版本来源、原 ARM 和 fresh 库存历史。参考工作目录及复制目录须保持未创建。目标计划发布后仍需接通并验证运行登记、正式恢复及业务验收。

## 非生产重建

MySQL 重建预检接受覆盖清单中精确库名的库级授权。每个迁移连接须具备目标库所需的 DDL/DML 权限，同一物理服务器的持锁连接也须对它负责的每个库具备 `SELECT`、`CREATE` 和 `DROP`。权限核对通过后才进入资源写入阶段。

`ryframe-reset` 只在显式 `bin-reset` feature 下编译，生产镜像不复制该二进制。先用 `git rev-parse HEAD` 查看完整提交，再将 40 位 Git SHA 填入所选明文配置的 `reset.source_sha`；`plan` 必须提供该字段及非秘密轮换声明 `reset.credential_version`，后者为 1～64 位 ASCII 字母、数字、点、下划线或连字符，任一 reset 相关密码或私钥轮换时必须同步变更。`reset.services_stopped` 默认 `false`，`plan` 不要求它为 `true`，也不要求 `reset.state_directory`。只有 `execute` 必须先停止 API、Worker 和 scheduler，声明 `reset.services_stopped=true`，显式提供绝对目录 `reset.state_directory` 并完成全部只读预检。管理员密码统一在 `[bootstrap] admin_password` 中明文显式提供并满足复杂度策略；普通种子用户 `user` 使用统一初始密码 `123456`，无需额外提供普通用户密码。重建验证同时核对 `admin`、`user` 的密码哈希及 `password_change_required=true`，任一不符均失败关闭；两个账号首次登录后都必须修改密码。

```powershell
git rev-parse HEAD
$env:APP_ENV = "test"
cargo reset -- plan --config-dir ../ryframe-reset-config
cargo reset -- execute --config-dir ../ryframe-reset-config --plan-hash <sha256> --confirm-reset <exact-confirmation-phrase>
```

`reset plan` 会真实读取、校验应用配置，生成绑定来源与资源范围的清单及哈希，不执行破坏性重建，也不能替代 `execute` 的 ownership 预检；它不属于不读应用配置的离线 `control export-sql --plan` 或 `adopt --plan`。reset 内部物理别名去重只避免重复处理同一物理数据库，不放宽独立租户目标的共库限制；仍须遵守[数据指南](data.md#租户路由)中的 schema 隔离约束。执行顺序固定为对象前缀、Redis namespace、物理数据库、控制 baseline、租户 baseline、验证和锁释放。清单、ledger 和 report 不包含秘密；普通阶段失败且锁释放成功时，只允许使用同一清单和 ledger 续跑。生产环境在读取配置或访问外部资源前永久拒绝。`cargo reset` 是使用者直接入口；框架维护工具 `cargo xtask data reset` 消费同一任务图并要求显式 `--write`。排障时不要混用两种入口的状态目录，也不要通过删除 ledger 启动新的重建。

报告的 `status` 区分 `completed`（本次完成）、`reused`（复用同一 ledger 的原完成）和 `failed`。`completed_at` 记录锁释放后持久化的原完成时间，重复执行不更新该值，也不重新运行预检或破坏性阶段；`reported_at` 仅表示报告生成时间。锁释放失败、中断或缺少完成证据时，重复执行仍失败，必须人工核对服务停止状态、MySQL 环境锁及清单内精确资源，保留原始失败记录。在 `reset.state_directory` 中选择一个新的绝对状态目录表示一次新的维护操作，目录路径不得包含点号跳转、链接或重解析点；必须重新生成并核验 plan、ownership 和资源范围，不得通过删除或覆盖旧 ledger 触发重置。当前开发版本使用 manifest v5、ledger v5 和 report v2；清单使用非秘密配置引用 `password_config_key`、`access_key_config_key`、`secret_key_config_key`，并保留 `credential_version` 的轮换绑定。旧格式明确拒绝，不自动转换或重放，旧本地证据继续保留。

## Cargo 工具来源核验

原生导出核验分别比较 API/Worker 产品分域与当前构建上下文，并复核显式登记的来源审计。工具或文档变化不会把旧产物改记为新源码构建；通过复用校验后仍保留原构建快照与指纹。运行窗口结束时重新核对完整执行来源和冻结产物，生产者清单按 PID 与创建时间去重，不能用不同名称重复登记同一代次。完整来源采集只在本次观察内固定规范根，逐文件保留普通文件、链接、规范路径和摘要检查；成功或失败后都重新核对根路径与目录状态，不跨观察或请求复用。

直接执行 Cargo build、check 或 test 时，xtask 构建脚本每次重新核对当前 Cargo 工作区根与完整来源文件集，包括新增和删除的文件。编译快照包含规范化的根路径及文件摘要；跨工作树即使内容相同，也属于不同的编译来源。工具的 describe 只读取编译时固化的来源，不在运行时重算。

生成内容不变时保留生成文件的内容及修改时间；内容改变时完整写入临时文件并安全替换。构建脚本保留完整文件与目录监视集，并监视一个不生成的 OUT_DIR 路径，使 Cargo 每次执行核验。若该检查路径已存在（包括链接），构建会拒绝并提示核对后精确清理该缓存对象或仅清理 xtask，不自动删除未知对象。当前工具链实测同根同内容的 build、check、test 仍执行构建脚本并重新编译 xtask，即使生成快照的摘要与修改时间保持不变；不能据此保证增量构建不重编译。共享 target 的跨工作树切换会更新根身份并重新编译。并发回归环境中的耗时仅供参考，独占性能需另行测量。

构建脚本还固化并核对自身与来源遍历 helper 的编译输入字节。共享缓存中的脚本若与当前根这两个文件不符，将拒绝构建并要求仅清理 xtask 后重建；不会读取当前文件后将其摘要伪装成脚本编译来源。

受管 origin 工具的完整来源核验由同一受保护二进制的私有 `__frontend-origin verify-sources` 执行。标准输入只接受 `format_version` 为整数 `1`、`root` 和 `sources` 三个字段，上限为 4 MiB；根必须是编译时同一真实且无链接或重解析点的目录，来源绑定必须与编译列表和当前完整发现集合一致，不增加排除目录。每个普通文件按 16 MiB 上限双读完整字节，并核对读取前、打开句柄、关闭后的真实设备/文件编号、大小和 mtime/ctime；Windows ctime 为创建时间，Unix ctime 保留纳秒。返回值仅含固定版本、种类、文件数及覆盖有序绑定与文件/根身份的 SHA-256 聚合 token，不含源码字节。原生消费者在发布结果前复核固定工具绑定、程序身份和完整来源。需要更新工具绑定时，在编译根使用同一程序执行 `__frontend-origin prepare --write`，保留既有失败证据。

前端构建证明由 Rust 私有 `__frontend-build-contract --frontend-dir <abs>` 统一核验：标准输入精确包含 `frontend_inventories` 与 `backend_inventories` 两数组，上限 16 MiB；响应仅含三域及公开配置、身份和完整产物描述。Node 从正式 consumer context 取得固定工具绑定。Rust 只读核对绑定、编译根和完整来源闭包，持有来源、配置与产物保护直到响应发布完成。普通恢复、B0 和原生静态站点在 Rust 内直接核验构建收据，启动探针清单返回前复核文件和成员集合。协议只传配置摘要，dist 大文件使用流式摘要，16 MiB 文档限额适用于协议文档。

共享缓存回归的隔离 Workspace 从真实 TOML 派生包属性、所需依赖、lint、profile 和 patch，源路径转为原 Workspace 的绝对路径。先离线解析种子锁并拒绝任何包版本、来源或校验和变化，之后构建均使用 `--offline --locked`。仅隔离宏副本和既有 `patch_route` 消费测试追加字面量 probe，实际执行 A→同字节 B 输出 A→改写 B 输出 B→返回 A 输出 A；产品宏不增加测试导出。

`ryframe-macro` 每次 Cargo 执行都会重新核对包内 `src/**/*.rs`、`Cargo.toml` 和 `build.rs` 的相对路径及字节摘要；新增、修改、删除源码均改变编译输入。同字节跨工作树不因绝对路径变化而改变指纹，同内容生成文件保持 mtime，但不保证 Cargo 不会再次编译。缺失监视哨兵若被其他对象占用则拒绝构建，不自动删除；脚本自身编译来源不匹配也明确失败。独占共享 target 时可运行 `cargo test -p ryframe-macro --test source_cache -- --ignored --exact shared_target_rechecks_macro_sources_across_roots`；运行进程须显式设置标准 Cargo `CARGO_TARGET_DIR` 为同一绝对缓存目录。回归在 `.local-tests` 保留 A/B 源码、Cargo JSON、错误和耗时，检查 build/check/test、返回 A、同内容 mtime 与增改删，耗时只有独占环境才可用于性能比较。

## 发布门禁排障

后端日常 CI 的 `Plan & Preflight` 作业执行 `cargo xtask check ci preflight --part checks`。`Preflight Python` 使用同一后端提交和 Plan 实际检出的前端提交，运行 `cargo xtask check ci preflight --part python`，包括解释器核验和完整 Python 测试。它与其余依赖 Plan 的门禁并行。计划启用预检时，`Required` 将 Python 作业失败、取消或跳过判为失败。排查时查看独立 Python 作业的日志和两仓检出记录。

协调版本 tag 同时触发双方日常 CI 和 Extended CI。后端的 core 与业务 crate 全栈任务均检出配套前端 tag，产物内记录双方精确 SHA、run ID 和 attempt；业务 crate 产物另附从干净源码生成隔离工作树的收据。Release 通过 `cargo xtask check release source` 核对双端 tag、版本与契约来源，通过 `cargo xtask check release ci` 在校验阶段和实际创建 Release 前分别检查四组最新运行、两套全栈产物及必需 job；缺失、失败、取消、超时、错误 SHA、跳过必需 job 或最新重跑未成功都会阻断发布。全栈产物按声明身份唯一选择 `extended-ci.yml`，使用对应源码 SHA、run ID 和 attempt，缺失或重复声明时拒绝，不依赖证据顺序或工作流数量。两个入口直接由 Rust xtask 执行，不依赖 Python，并支持 `--plan` 只查看实际任务图。 Release 在检出配套前端前通过 Rust `frontend-source` 从工具文件或标准 GitHub owner 身份选择仓库，配置方式见[开发指南的框架维护检查](development.md#框架维护检查)。校验阶段冻结该仓库、双方精确提交及 tag 对象，发布阶段复用冻结结果，不重新读取仓库选择。前端仍按显式发布 tag 检出，仓库配置不会改变双方 tag 必须匹配及精确来源核验。

检查 Release 保存的 `release-ci-evidence.json` 与 `release-final-evidence.json`，按其中 run ID 和 attempt 定位失败。默认总等待上限为 5400 秒，GitHub API 分页和单次调用共享截止时间。修复后对同一目标源码重跑相应 CI，再重新运行 Release；不移动已有 tag，也不以其他提交的成功结果替代。发布资产仍为 GitHub 源码归档。
