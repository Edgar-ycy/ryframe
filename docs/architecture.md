# 架构与扩展位置

[English](architecture.en.md)

MQTT、邮件和乐观锁等插件属于后续路线：优先采用按需 Rust crate 与编译期装配，未引入、未启用时不增加服务要求。MQTT、邮件按集成能力设计，乐观锁按持久化扩展设计；届时同时处理依赖、配置、生命周期、权限菜单、租户隔离、能力目录及套餐同步。当前不实现插件系统，不添加空接口、动态加载开关或占位依赖。

本文帮助业务开发者理解一次请求如何经过 RyFrame，并选择合适的扩展位置。

## 运行结构

```text
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

HTTP 层把请求解析为明确的 DTO，再调用 application 用例。用例通过端口访问数据库、Redis 和对象存储，因此业务流程可以在不依赖具体连接实现的情况下测试。API 与 Worker 使用同一组应用服务。组合根通过 `APP_ENV=dev|test|prod` 选择必须存在的 `config/app.<环境>.toml`，与公共 `config/app.toml` 合并；密码、JWT、监控、Redis 和存储凭据使用明文字段，初始管理员密码来自 `[bootstrap] admin_password`。根字段 `snowflake_worker_id` 在同一部署的各实例间必须不同。应用 `--config-dir <目录>` 默认 `config`，相对当前工作目录解析；生成器的 CLI、Builder `.configuration_directory(path)` 及可选 `[generator] config_dir` 相对后端 Workspace 根目录解析。生成器优先级为显式 CLI/Builder、本机 `.local-tests/tools.toml`、公共 `config/tools.toml`、`workspace/config`；工具文件不接受环境变量覆盖。本地化通过 TOML 的 `app.locales_dir` 指定，默认 `locales`，普通应用相对进程当前工作目录解析，不读取 `APP_LOCALES_DIR`；生产加载资源失败时严格拒绝启动，开发环境允许内嵌回退。配置保存后热切换、失败时回滚旧代次是已确定的 dev 行为要求，当前文件配置链路尚未实现，不能作为现有能力依赖。部署四角色模板与私有配置目录已集成并通过资产结构及 CI 工具测试；真实容器、镜像和整链仍未验收，局部检查不代表整体门禁。

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

RyFrame 官方源码不内置业务示例。开发者用 `cargo generate -- new-business <业务包名> --write` 在 `crates/` 下创建业务 crate，例如 `order-business`、`inventory-business`，通过 `ryframe-sdk` 使用框架扩展接口。省略 `--write` 预览；脚手架原子写入工作区成员、组合根依赖和唯一注册表。一个业务 crate 默认同时包含模型、用例、Repository、Handler、路由和迁移；业务复杂后再按实际边界拆分。

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

外部运维工具负责生成一致的数据库与对象备份。application 的备份用例拥有登记、恢复状态机和事务；控制库 Repository 保存清单及演练记录，租户库实现目标与数据校验，adapters 实现文件摘要、对象摘要和就绪探测。维护 CLI 负责读取显式配置、装配端口并打印结构化结果。API 后台采集器只读取汇总状态，向 Prometheus 输出固定维度指标。

恢复依次经历开始、数据验证、业务与运行验证；失败为终态。目标必须具有独立 scope 和 ownership，数据校验比较明确登记的物理数据库、schema、租户关系、表摘要与对象摘要。业务证明绑定演练记录和精确前后端源码，只有数据校验、完整浏览器场景、API/Worker 就绪以及恢复时间目标都满足时才登记成功。具体操作见[运维指南](operations.md#备份失败或过期)。

## 非生产重建

`ryframe-reset` 只在显式 `bin-reset` feature 下编译，生产镜像不复制该二进制。先用 `git rev-parse HEAD` 查看完整提交，再将 40 位 Git SHA 填入所选明文配置的 `reset.source_sha`；`plan` 必须提供该字段及非秘密轮换声明 `reset.credential_version`，后者为 1～64 位 ASCII 字母、数字、点、下划线或连字符，任一 reset 相关密码或私钥轮换时必须同步变更。`reset.services_stopped` 默认 `false`，`plan` 不要求它为 `true`，也不要求 `reset.state_directory`。只有 `execute` 必须先停止 API、Worker 和 scheduler，声明 `reset.services_stopped=true`，显式提供绝对目录 `reset.state_directory` 并完成全部只读预检。管理员密码统一在 `[bootstrap] admin_password` 中明文显式提供并满足复杂度策略；普通种子用户 `user` 使用统一初始密码 `123456`，无需额外提供普通用户密码。重建验证同时核对 `admin`、`user` 的密码哈希及 `password_change_required=true`，任一不符均失败关闭；两个账号首次登录后都必须修改密码。

```powershell
git rev-parse HEAD
$env:APP_ENV = "test"
cargo reset --help
cargo reset -- plan --config-dir .local-tests/reset-config
cargo reset -- execute --config-dir .local-tests/reset-config --plan-hash <sha256> --confirm-reset <精确短语>
```

`reset plan` 会真实读取、校验应用配置，生成绑定来源与资源范围的清单及哈希，不执行破坏性重建，也不能替代 `execute` 的 ownership 预检；它不属于不读应用配置的离线 `control export-sql --plan` 或 `adopt --plan`。reset 内部物理别名去重只避免重复处理同一物理数据库，不放宽独立租户目标的共库限制；仍须遵守[数据指南](data.md#租户路由)中的 schema 隔离约束。执行顺序固定为对象前缀、Redis namespace、物理数据库、控制 baseline、租户 baseline、验证和锁释放。清单、ledger 和 report 不包含秘密；普通阶段失败且锁释放成功时，只允许使用同一清单和 ledger 续跑。生产环境在读取配置或访问外部资源前永久拒绝。`cargo reset` 是使用者直接入口；框架维护工具 `cargo xtask data reset` 消费同一任务图并要求显式 `--write`。排障时不要混用两种入口的状态目录，也不要通过删除 ledger 启动新的重建。

报告的 `status` 区分 `completed`（本次完成）、`reused`（复用同一 ledger 的原完成）和 `failed`。`completed_at` 记录锁释放后持久化的原完成时间，重复执行不更新该值，也不重新运行预检或破坏性阶段；`reported_at` 仅表示报告生成时间。锁释放失败、中断或缺少完成证据时，重复执行仍失败，必须人工核对服务停止状态、MySQL 环境锁及清单内精确资源，保留原始失败记录。在 `reset.state_directory` 中选择一个新的绝对状态目录表示一次新的维护操作，目录路径不得包含点号跳转、链接或重解析点；必须重新生成并核验 plan、ownership 和资源范围，不得通过删除或覆盖旧 ledger 触发重置。当前开发版本使用 manifest v5、ledger v5 和 report v2；清单使用非秘密配置引用 `password_config_key`、`access_key_config_key`、`secret_key_config_key`，并保留 `credential_version` 的轮换绑定。旧格式明确拒绝，不自动转换或重放，旧本地证据继续保留。
