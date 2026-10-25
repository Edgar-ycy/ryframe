# 运维

[English](operations.en.md)

## 部署与发布

生产镜像使用固定摘要的 Distroless Debian 13 运行层，只包含 API、迁移、Worker 及其运行依赖，不带 shell、包管理器、`curl`、生成器、文件维护或 reset。API 与 Worker 由自身的 `--healthcheck` 命令探测本机 `/readyz`。运行镜像仅设置应用环境变量 `APP_ENV=prod`，在工作目录 `/opt/ryframe` 下默认读取必须存在的明文 `config/app.prod.toml`，可用 `config/app.toml` 提供公共默认值；数据库、JWT、Redis、对象存储和监控凭据直接填写字段。真实文件保存在受控的忽略目录，并只读挂载到各实例容器的独立配置目录，CLI 用 `--config-dir <directory>` 选择；`scope_id` 保持稳定，各实例根字段 `snowflake_worker_id` 必须不同。镜像在部署配置中使用完整 `image@sha256:<digest>`，维护命令如提供镜像摘要选项则按其 CLI help 指定，不再使用项目环境变量传递。API、Worker、控制库迁移、租户数据迁移四角色模板及各自私有配置目录已集成，资产结构与 CI 工具测试通过；真实容器、镜像和部署整链仍未验收。若使用本地存储后端，在所选角色配置文件的 `object_storage.local_base_dir` 字段设置存储目录，不使用环境变量覆盖。下列命令是后续核对挂载、固定镜像和实际启动的示例，不代表运行验收已通过：

镜像构建使用 Docker 的 `SOURCE_COMMIT` 构建参数绑定 40 位小写提交 SHA。Dockerfile 校验后将它写入构建容器内的受管 `.local-tests/build/source-commit`，Rust 构建脚本只读取该文件；它不属于应用环境变量，也不会进入运行镜像。仓库内正式维护构建和恢复构建由 Rust/Python 协调器独占创建同一受管文件，完成后核对内容再清理；并发、漂移或未知清理结果会失败并保留现场。直接运行普通开发构建且没有该文件时，版本信息明确为 `development`，不能用于正式恢复证明。
```powershell
$sourceCommit = git rev-parse HEAD
docker build --build-arg "SOURCE_COMMIT=$sourceCommit" -f deploy/Dockerfile .
```
```powershell
docker compose -f deploy/compose.prod.yml config
docker compose -f deploy/compose.prod.yml up -d
```
四角色部署顺序是先更新控制库与租户库，再启动 Worker 和 API；模板结构检查不替代真实环境的启动和升级验收。启动后检查 API `/livez`、`/readyz`，以及 Worker 健康端口的 `/readyz`。Nginx 只代理 API 的 edge 网络端口，MySQL、Redis 和对象存储保持在内部网络。直接在主机运行时，先创建空数据库并设置 `APP_ENV` 与配置，再按 `control up`、`control verify`、`tenant-data up --target shared-control`、`tenant-data verify --target shared-control`、`cargo serve` 的顺序执行。默认单租户也有控制库和 `shared-control` 两套迁移账本；`verify` 只校验，新空库必须先 `up`。完整步骤见[数据指南](data.md#全新环境初始化)；完整 SQL 导入同一物理控制库后，直接执行 `control verify` 与 `tenant-data verify --target shared-control`，无需补 `up`；其他独立 MySQL 租户目标仍须各自 `tenant-data up/verify`。生产默认且只允许 `jobs.mode="external"`，不能只启动 API；为 API、Worker 分别准备含 `app.prod.toml` 的私有目录，在两个终端均显式设置 `APP_ENV=prod`，以各自的 `--config-dir <directory>` 先启动 Worker 再启动 API；两角色使用不同 `snowflake_worker_id`，指向同一控制库和 scope。 本机沿用 README 的忽略目录时，本文默认 `config` 的 Cargo 别名示例须在 `--` 后传 `--config-dir .local-tests/dev-config`，例如 `cargo migrate -- --config-dir .local-tests/dev-config control verify` 或 `cargo serve -- --config-dir .local-tests/dev-config`；后续校验、导出和运行继续选择同一目录，避免连接另一份未初始化配置。 完整 SQL 的结构断言失败会输出迁移、阶段与类别；保存原始结果和现场，不因账本仍为空就认定 DDL 已回滚。先只读核对，禁止自动重放或补写完成标记。

当前版本只接受全新生产库。已有真实生产旧库需要先设计并演练非破坏升级方案，不能使用 reset 代替升级。 生产 API 还必须通过 TOML 的 `app.locales_dir` 提供完整 `zh-CN.toml` 与 `en-US.toml`；默认目录为 `locales`，普通应用相对进程当前工作目录解析，不读取 `APP_LOCALES_DIR`。缺失或加载失败会严格拒绝生产启动；开发环境的内嵌回退不能替代生产资源部署。

## 非生产重建

重建仅用于非生产环境。完整配置、双阶段命令、账本及失败恢复规则见[架构说明：非生产重建](architecture.md#非生产重建)；生产环境永久拒绝该入口。

## HTTP 5xx 与高延迟

应用日志过滤级别使用配置中的 `logger.level`；`RUST_LOG` 不会覆盖该级别。排查时修改所选配置文件并重启对应进程。 先按请求 ID 检查 API 日志和 trace，再检查数据库、Redis、对象存储与 Worker。确认是单一依赖故障后按降级策略处理；不要通过扩大重试放大故障。

## Redis 降级

检查连接模式、TLS、ownership marker 和 scope 前缀。授权缓存不可用时必须失败关闭或按配置禁用缓存；禁止清空共享 Redis DB。

## Redis 与对象存储约定

- Redis `timeout_secs` 统一约束连接、响应和事务，范围 1–60 秒；兼容 S3 后端使用 `object_storage.request_timeout_secs`，范围 1–300 秒。不得在调用点叠加无上限重试。
- 就绪探针只执行 Redis `PING` 和对象存储私有桶检查，并受独立短超时保护；`required` 依赖失败才阻断就绪，可选依赖明确进入降级状态。
- `ryframe_connector_operations_total` 与 `ryframe_connector_operation_duration_seconds` 只使用 connector、固定 operation 和 result 三类低基数标签。端点、租户、Redis 键、桶名和对象键禁止进入指标标签。
- Redis 与 S3 配置的 `Debug` 输出会隐藏密码、访问密钥和客户端私钥。S3 非成功响应只保留稳定操作名和 HTTP 状态，直接丢弃远端响应体；传输错误移除请求 URL。文件维护错误只携带内部文件 ID，不记录桶名、对象键、Authorization、签名或负载。
- 确定性测试使用本地实现或离线协议样本；真实协议测试必须显式启用，并使用唯一 schema/namespace 与精确清理，禁止 `FLUSH`、`KEYS` 和模糊对象前缀删除。

## Refresh Token 重放与异常账号恢复

立即撤销 token family 和当前会话，核对用户授权版本、客户端 IP 与审计事件。确认泄露后轮换相关凭据并保留证据。旧开发版普通账号无法登录且密码未知时，使用[显式账号恢复流程](api.md#异常账号恢复)；不要重复初始化或直接修改密码哈希。停用与改密要求独立，重置成功不会自动启用账号。

## 依赖与任务排障

| 现象 | 核对与处理 |
| --- | --- |
| 限流拒绝 | 区分租户配额、账号策略和基础设施故障。只在确认业务容量允许后调整配置，不删除限流键或绕过租户边界。 |
| 数据库副本与读回退 | 检查复制延迟、连接健康和 read consistency。授权、任务、导出与状态变更必须回到主库；不要把强一致读临时改为副本读。 |
| 后台任务 | 检查 lease、heartbeat、attempt、公开错误和死信状态。数据库会拒绝重复领取；修复原因后使用受控重试，不直接改任务终态。 |
| 定时调度 | 核对表达式、时区、misfire 策略和下一次运行时间。避免同一业务任务由多个 scheduler 重复创建。 |
| 消息中心投递 | 检查 outbox、分发任务、目标受众和多语言参数。先恢复幂等消费，再处理积压；不要跳过租户或权限过滤。 |
| OpenTelemetry 导出器 | 导出失败不应阻断业务请求。检查 endpoint、TLS、超时和队列丢弃指标，恢复后确认 traceparent 继续传播。 |
| 数据库连接容量 | 按控制库、租户目标和 Worker 分别统计池使用量。优先处理泄漏、慢查询和错误并发，再调整池上限；总连接预算不得超过数据库限制。 |
| 磁盘容量 | 检查 `target`、日志、临时 XLSX、上传暂存和本地对象目录。只删除已确认的构建缓存或已过期产物，保留最新调试符号和 reset ledger。 |

## 备份失败或过期

业务表复制目录由原生业务目录加载器从执行后端的显式 Workspace 成员读取，统一核验受管生成资产、内容摘要、模块归属及完整表描述；返回表顺序保留 Workspace 与机器目录的声明顺序。业务迁移账本的完整列和类型来自同一 Rust 校验入口读取的正式 SDK DDL。验收调用方通过显式绑定当前工具的只读协议取得结果；Python 桥接不再单独解析或接受片段目录。目录或归属被拒绝时，应核对执行工作树，并通过正式生成入口修复受管资产；账本结构被拒绝时，应核对执行树的 SDK 版本及 DDL。不能过滤未知表或绕过校验继续复制。

停止依赖无效备份的迁移操作，检查目标、校验和、采集时间和保留策略。恢复后重新生成并验证备份，不手工标记为成功。外部工具负责备份与还原字节，RyFrame 只负责清单采集、登记、恢复验证、状态和告警。

### 参考环境与一致性

参考演练采用系统租户和 10 个普通租户，覆盖共享与独立数据目标，至少 10 万条代表性业务记录和 1 GiB 对象数据。外部运维调度每 12 小时执行完整备份，保留至少 7 天。演练报告必须记录 Windows 机器、MySQL/RustFS 版本、存储和网络条件；24 小时恢复点、60 分钟恢复时间是该规模下需要实际验证的目标。

开始采集前暂停业务写入，停止 API 写入入口、Worker、scheduler 和消息等生产者。在整个清单采集、数据库转储和对象复制期间保持静止，避免把多个时间点的数据登记为一致备份。数据库按完整主键顺序计算摘要，对象采用流式 SHA-256；清单包含源码 SHA、schema 指纹、物理数据库身份、租户 placement、对象前缀、字节数和摘要，不包含密钥。

复制后的管理员会话使用独立的当前凭据文件。先通过正常账号流程完成首次改密，再在后端忽略目录中创建私有文件，例如绝对路径 `<backend>/.local-tests/clone/administrator.toml`：

```toml
[administrator]
tenant_id = "system"
username = "admin"
password = "<current-password-after-first-change>"
```

post-copy 注册请求的 `source_admin` 仅包含 `subject_id`、`tenant_id`、`username`、`client_address` 四项元数据；另以 `administrator_credentials` 登记该 TOML 的 `path`、`bytes` 和 `sha256` 文件绑定，不把密码写入请求、公开登记或收据。文件中的租户和用户名必须与元数据一致，文件及各级路径不得经过链接或越出当前后端的 `.local-tests`。注册和会话启动在读取前后核对文件绑定；内容或绑定变化、字段错误和 TOML 解析失败均拒绝继续，诊断不回显文件原文。登录后还要求权威 `SessionContext` 顶层的 `password_change_required` 明确为布尔值 `false`，为 `true`、字段缺失或类型错误时拒绝进入业务动作。此密码独立于 `bootstrap.admin_password`，工具不自动改密、不回退初始化密码，也不重放业务写入。未完成首次改密时，先完成明确的账号流程，不能将初始化密码作为当前凭据登记。

继续通过 `cargo xtask check recovery clone post-copy --run-dir <run> --operation register --request <request.json> --write` 显式登记；后续产生写入的操作仍需 `--write`。`api_environment` 绑定文件仅允许 `{"environment":{"APP_ENV":"test"}}`，不再传递管理员凭据或其他项目配置；地址、Worker 模式和 CORS 等设置来自各自绑定的应用 TOML。amendment 仅可在既有停止证明和失败核对流程后重新绑定环境选择文件，不能借环境字段修改项目配置。

### 采集和登记

Windows 历史生产者的停止核验先读取内核身份：进程不存在，或同一 PID 的创建时间严格晚于登记时间，才证明旧代次已经结束。仅在内核查询返回拒绝访问（错误码 `5`）时，使用受管 Rust 子进程查询本机 CIM；查询超时预算为 15 秒。CIM 保留微秒精度，创建时间必须严格晚于旧时间加 10 个 100 ns tick 的边界。同一代次、相邻精度边界、重复身份、未知时间或查询失败都拒绝登记，不再回退 PowerShell/PDH，也不会向占用该 PID 的进程发送信号。 若错误包含 CIM 的 HRESULT，先用 `Get-Service Winmgmt` 查看本机服务状态，并检查 `ROOT\CIMV2` 的读取权限。修复后重新执行原核验命令；保留已有阶段结果和局部文件，按实际状态恢复，不手工改写“已停止”或自动重放数据写入。`Get-Service Winmgmt` 仅用于人工排障，不是运行时回退。

在 Windows 维护机通过 `cargo xtask data` 执行备份和恢复登记；配置和密钥放在受控的忽略目录。该维护能力不包含在生产运行镜像中。
```powershell
cargo xtask data --help
cargo xtask data backup inventory --output <absolute-new-file-in-backend-workspace/.local-tests> --source-sha <exact-backend-sha> --quiesced-at <writes-quiesced-at-rfc3339>
```
`backup inventory` 与 `target inventory` 都是显式选择的只读库存操作，只在当前后端 `.local-tests` 内以 create-new 语义原子创建新证据文件，不修改数据库、对象或登记状态，因此不使用 `--write`；输出文件必须事先不存在。随后由外部工具按清单中的精确数据库、表和对象前缀完成复制与校验，再补齐 `id`、`completed_at`、`retention_until` 和 `artifacts`，形成 `BackupManifest`。每个 `db:<logical-target-key>` 与 `objects:<bucket>` 至少对应一个文件产物，登记其 `relative_path`、`bytes`、`sha256`；相对路径不得越出备份根目录。结构定义见 `crates/ryframe-application/src/ports/backup/manifest.rs`。

需要检查尚未分配租户的目标或初始化前后状态时，使用 `cargo xtask data target inventory --target <configured-target-key> --output <absolute-new-file-in-backend-workspace/.local-tests>`。该命令只连接明确配置的数据库，校验 schema 和精确全表目录，并在只读事务中计算完整行摘要；`target.database` 保存业务表与 placement，`target.preserved_tables` 单独保存 ownership、迁移账本和备份恢复登记表。它不访问对象存储、不修改目标，也不把未使用的目标遗漏为“空清单”。控制库与独立目标之间的一致性仍要求外部停止所有生产者；单份目标清单不表示备份或恢复成功。

```powershell
cargo xtask data backup register --manifest <absolute-manifest-in-backend-workspace/.local-tests> --backup-root <registered-external-absolute-backup-directory> --write
cargo xtask data backup status
```
登记会重新读取文件验证大小及 SHA-256，缺失或损坏会登记为无效并返回失败。同一备份 ID 只允许重复验证相同清单，不允许替换数据。登记成功表示已校验备份文件，尚不能表示恢复成功。状态查询只读取控制库，不依赖对象存储当前是否可连接。

### 隔离恢复与验收

1. 准备全新的隔离数据库和对象前缀，先按当前基线初始化并建立目标 ownership。逻辑 target key 保持一致，物理数据库、scope、JWT 密钥和 Redis namespace 必须与原环境不同，Worker 使用 external 模式。维护命令的 `--config-dir <directory>` 选择源侧登记库配置，`--restore-config-dir <directory>` 只选择隔离恢复目标配置；核对两份明文文件中的物理身份、scope 和端点，避免混用源与目标。应用相对配置目录以命令当前工作目录解析，建议使用绝对路径。产品 `backup-register`、`restore-begin` 和后续验证状态均写入源侧登记库。
2. 按 `crates/ryframe-application/src/ports/backup/restore.rs` 的 `RestorePlan` 填写完整目标、故障时间、前端 SHA、API/Worker 就绪地址和新对象前缀。物理数据库身份必须匹配，不能指向任一原业务库。
3. 在开始外部还原之前执行 `restore-begin`。重复相同 plan 不重置计时。外部还原仅覆盖该 plan 中已核验的隔离资源，保留目标自己的 ownership 和备份登记元数据；数据库转储只含清单中的业务表，不执行指向源库的 `USE`、数据库创建或跨库语句。对象路径仅替换 scope 前缀，不能复制源 ownership 标记。
4. 停止目标业务写入，执行 `restore-verify-data`。它重新验证备份文件、当前 schema、完整表内容、租户关系和完整对象集合，缺失、多余、损坏或错误 ownership 都会失败。
5. 使用全新 Redis 临时状态启动 API、Worker 和前端，完成真实浏览器业务验收，再提交绑定演练 ID、plan hash、源码 SHA、scope 和时间范围的 `RestoreBusinessProof`。`restore-verify` 再检查两项就绪探针，只有数据与业务验证均通过且总计时不超过 60 分钟、备份恢复点距离故障不超过 24 小时才记录成功。

依次运行 `cargo xtask data restore begin --plan <absolute-plan> --output <absolute-new-running.json> --restore-config-dir <absolute-target-configuration> --write`、`cargo xtask data restore verify-data --id <drill-id> --backup-root <absolute-backup-root> --output <absolute-new-data-verified.json> --restore-config-dir <absolute-target-configuration> --write` 和 `cargo xtask data restore verify --id <drill-id> --proof <absolute-business-proof> --tests-receipt <absolute-test-details> --runtime-receipt <absolute-runtime-receipt> --target-plan <absolute-target-plan> --runner-root <absolute-clean-test-runner-worktree> --restore-config-dir <absolute-target-configuration> --write`。计划、证明、收据和输出必须位于当前后端 `.local-tests` 子目录；备份根、隔离配置和干净 runner 可以使用已登记的外部绝对目录。所有路径都不能经过符号链接或重解析点；输入文件和目录必须存在，输出父目录必须存在且目标文件必须不存在。这些条件及 `--write` 在启动维护二进制和连接资源前核验。业务成功后先在同目录写入并同步临时文件，再以 create-new 语义原子发布并重读规范字节、JSON 和路径。参数错误、业务失败或已有目标不会创建或截断成功记录；若业务写入成功后发布或复核失败，保留登记库状态和任何已发布文件，先运行只读状态核对，不得盲目重放。`restore-begin --output <new-file>` 发布的运行中记录继续传给外部恢复阶段。

原生 fresh-target 静态校验只从所选明文配置文件读取连接、凭据、Redis 和 Worker 地址，环境映射仅用 `APP_ENV=test` 选择测试文件。fresh 目标请求必须用绝对 `config_dir` 明确绑定执行后端内已存在、无链接且目录名为 `config` 的配置目录；缺失、相对路径、旧 `configuration_directory` 键或双键均拒绝，不回退默认目录。嵌套执行树不能使用协调后端的其他配置目录；`app.test.toml` 必须存在，公共 `app.toml` 可省略。旧配置目录、端点、密码和 `_FILE` 环境覆盖不参与本入口的配置或摘要，旧对象凭据变量名也不能代替请求中的明确凭据字段。静态校验通过不能代替进程、端口、实际 ownership 和完整恢复链路的验证。 原生源对象采集和目标对象复核使用 Rust S3 读取端口，凭据来自已绑定文件，HTTP 使用 AWS-LC TLS 和 SigV4。目标清单逐页读取并核对完成状态、scope、重复 key 和 continuation token；对象 GET 使用明确的长度上限、条件 ETag/版本和独占创建的文件，禁止重定向、自动重试、自动解压和部分内容响应。读取中断后保留部分文件，须核查该 attempt 的失败证据，不能把残留文件当作成功导出。源导出的公开 `request.json` 和 `export.json` 只保留 S3 端点与区域，凭据保留在原始私有文件中；原文件的字节数和摘要仍独立绑定，并在复核时重新生成公开投影进行比较。私有源请求继续使用端点、区域与明确凭据四字段；对象读取端口只接收从同一请求文件、运行收据及 TOML 重建的公开来源和 mysql/mysqldump 两工具投影，原始私有文件不改写。原生读取不需要 AWS 子进程；恢复流程其余尚由 Python 采集或复制的步骤仍按各自预检核对工具依赖，不能仅凭原生读取通过就省略完整恢复验收。 fresh-target CLI 的审阅请求、工具集合、reset 清单、配置目录、首次启动历史、私有环境与预检收尾绑定由当前受管 Rust 工具通过有界标准输入核验；缺少工具绑定时直接报错，不通过 PATH 或 Cargo 猜测程序。CLI 私有环境文件只允许 `APP_ENV`、`TEMP`、`TMP`，后两者必须指向同一已存在的私有目录；连接、密码和其他项目配置写入绑定的 TOML 文件。 `cargo xtask check recovery fresh-target --operation prepare --workspace <new-workspace> --request <request.json> --environment <private-environment.json> --storage-run <registered-run> --write` 与同工作区的 `--operation resume-prepare --write` 由 Rust 原生执行，不启动 Python。prepare 只读核对四库不存在、五桶完整空前缀和 Redis 空 namespace／owner／sentinel，再发布本地准备证据。校验失败或中断后保留全部文件，先用 `status` 核对，再显式续作；不能自动重放、清理资源、删除旧收据或抢占残留控制锁。续作 intent 绑定精确前像，完整观察组仍逐项核验数量、工具、配置和进程代次；历史 prepare 原文不变，合法后继必须沿完整已登记账本证明，未知文件或漂移均拒绝。原始命令失败材料只留在已绑定私有 TEMP，不进入公开阶段收据。同一正式入口的 `--operation initialize --write` 和 `--operation resume-initialize --write` 也由 Rust 执行，不经 Python 或只读私有协议；保持工作区、存储运行与目标的锁顺序，先持久化 intent，再执行四库 CREATE、sentinel、reset 和十项迁移，最后采集完整库存。reset、迁移及库存命令都显式传入请求绑定的绝对 `--config-dir`，不回退到默认配置目录。首次初始化和库存续作要求 generation 整体相等，只有已核验的迁移续作允许登记后继；未知写入保留全部失败证据，不自动重放或 DROP。目标目录锁释放后仍持有真实目标 guard 发布完整文件快照。迁移 intent 绑定完整中断文件前像，原始失败只读收据按字节保留；已登记重启并确认 marker 重绑后，仅首个只读核验发生已严格绑定的长路径收据发布失败时允许一次显式后继，二次失败停止后续尝试。写命令成功但确认文件未发布仍按未知写入保留，禁止重放。Redis marker 重绑暂保留服务端 Lua 的单次 EVAL 原子语义；Rust 调用脚本不表示脚本已经替换。上述入口均不授予正式恢复资格，完整真实链仍需单独验收。 fresh-target 的 `--operation status` 在 Rust 内只读重验登记、完整存储追加历史、阶段文件树和控制器身份；未持初始化写锁的缓存发布 binding 查询也在原生账本内完成，不调用 Python 观察原语，不取得控制锁、不写业务或补齐收据。服务停止与部分写入恢复仍须使用原有显式入口，status 不授予重放许可。

发布本侧目标计划后、首次向 fresh target 写入数据库或对象前，执行 `cargo xtask check recovery runtime register --plan <reference-plan-json> --target-plan <target-plan-json> --output <new-runtime-registration.json> --write`，登记目标从未启动。登记入口在 ownership 控制锁内，写入前后核验目标运行目录没有 lifecycle、launch、进程树或未知文件，并证明 API、Worker、前端三个精确端口空闲；缺少进程收据本身不能作为停止证明。正式恢复执行器使用同一登记锁包住完整写入过程，并在每次数据库或对象写入前及退出时重新核验。登记文件绑定两个计划的绝对路径、大小和 SHA-256 及零进程观察；恢复收据绑定该登记文件，计划或运行现场变化后必须新建登记，不能覆盖旧文件。目标侧 ownership 与备份、恢复登记表必须完整保持初始化前像，不因登记操作获得例外。运行登记、完整资源像或任意产物漂移都会失败；失败记录保留后像复核结果，未知写入不能自动重放。

完整验收保留成功恢复及损坏或缺失备份失败演练的原始日志、校验结果、trace、截图和视频。失败不会被登记覆盖为成功，也不会自动操作原业务资源；`running`、`data_verified`、`succeeded`、`failed` 分别表示已开始、数据通过、全部通过与失败，超时未完成仍会触发告警。

### 生成恢复业务证明

使用 `cargo xtask check recovery inputs bindings --reference-plan <side-reference-plan> --target-plan <side-target-plan> --backup-receipt <same-backup.json> --record <data-verified.json>` 只读预览数据绑定；加 `--output <new-bindings-file> --write` 才原子发布。该入口从已核验记录和原备份清单推导唯一的 `record`、`manifest`，不接受人工摘抄或重组。前后端必须是清单绑定 SHA 的干净源码，使用 `cargo xtask check recovery runtime build --source-backend <absolute-backend-worktree> --expected-head <full-backend-sha> --source-frontend <absolute-frontend-worktree> --expected-frontend-head <full-frontend-sha> --output <backend-worktree/.local-tests/restore/build.json> --write --frontend-dir <current-tools-frontend>` 统一生成并核验实际 API、Worker 和前端生产构建收据。历史源码不需要提供当前 CLI；协调器使用当前干净工具调用目标工作树自己的 Vite，并在目标前端写入 `.vite/restore-build.json`。已有前端收据会经过相同的严格核验后复用，绝不覆盖或补造构建事实。当前后端构建收据使用 v2、前端构建收据使用 v3，分开绑定按产物角色计算的产品输入、验收工具输入、完整源码清单、实际工具链和有效构建参数。前端从收据保存的 `configuration.environment` 选择 `app.dev.toml`、`app.test.toml` 或 `app.prod.toml`，按公共 `app.toml`（可缺省）、所选环境文件（必需）的读取顺序绑定路径和摘要；同时绑定 `config/tools.json` 与 `.local-tests/frontend-tools.json` 的实际来源及优先级、受管 `.local-tests/build/source.json` 身份和完整 `dist` 清单。核验不依赖当前 shell 的 `APP_ENV` 或旧 `VITE_*`；真实构建要求合并后的 `api_origin` 为空，正式来源身份必须等于声明的精确干净 HEAD，且与 `dist/build-identity.json` 一致。旧格式仅保留为历史文件，不能用于当前恢复。

`runtime start` 必须同时传入 runtime registration、完整 target plan、双端来源、构建收据和数据绑定；它从 target plan 绑定的明文配置文件或原子配置快照启动 API、Worker、前端，并为每个角色先保存 operation intent，再用同一 Job Object 或进程组登记完整树。首次创建运行目录前还会在目录外写入唯一 creation intent，因此控制器在目录创建、状态写入或进程收据发布的任一切点退出后，都只能通过绑定 owner 和 generation 的 `runtime recover --write` 接管。每个 generation 只允许当前 operation ID 对应的日志、进程、完整树、成员、控制、结果、完成和 launch 收据；未知文件、目录或链接会阻断继续操作并保留现场。`runtime status` 只读派生真实的 `registered_not_started`、`running`、`degraded`、`exited`、`interrupted` 或停止状态；`runtime stop --generation <N> --write` 反向回收三棵树并证明端口释放。
三端就绪后，用 `cargo xtask check recovery runtime bind --source-backend <execution-backend-worktree> --source-frontend <frontend-worktree> --build-receipt <build.json> --launch-receipt <generation-directory/runtime-launch.json> --bindings <bindings-file> --output .local-tests/restore/runtime.json --write` 签发 v3 运行收据。普通候选的产品与执行后端必须是同一提交；B0 另外显式传入 `--adapter-contract legacy-stable-readiness-b0-v1 --product-backend <815c5eaf-product-worktree>`，执行来源必须是已登记的 `c05114bc` 适配提交。运行收据分别保存备份来源、后端产品来源、后端执行来源、适配合同、前端来源以及三进程 generation，不能用备份清单的源码 SHA 冒充运行二进制来源。该入口核对源码、二进制、进程创建身份、完整树、探针监听端口及实际返回的前端文件，配置目录正确或 HTTP 200 均不能单独充当来源证明。

在隔离验收配置文件中成组登记绑定文件、运行收据、严格目标计划、当前干净协调器源码路径、当前干净前端测试 runner 完整 SHA 和当前干净后端核验器完整 SHA，并绑定新 scope 与已启动的生产站点。字段和文件选择参数以配置事实源及 CLI help 为准，接线仍待整合核对；原数据身份从严格目标计划绑定的备份、同代 STOP/START 与 source-runtime 递归推导，再运行 `corepack pnpm check --stage browser --real --server preview`。目标计划和运行收据独立绑定实际产品前后端，因此 B0 产品前端可以与当前测试 runner 不同。首次登录前，测试 runner 使用工具文件中登记的已验证解释器绝对路径调用已核验后端的只读数据预检，完整重验目标计划、来源代次和血缘，并将运行收据的 launch 规范路径与摘要、lifecycle、构建、端点和三角色进程文件交叉绑定后，取得内存中的唯一文件绑定；预检不访问服务、不创建控制锁或 Python 字节码；预检失败不得进行登录、注销或业务请求，不自行抽查少量收据字段替代完整核验。测试开始及结束均复核来源；运行期间替换进程、修改源码或产物都会拒绝生成成功证明。各文件只保存摘要和资源归属，不包含密钥。

测试覆盖全部恢复核心场景并通过控制台、网络和 axe 断言后，结果写入 runner 的 `.local-tests/playwright-real/restore-<drill-id>.json`，测试明细和运行收据副本使用同一前缀相邻保存。证明明确记录备份来源、后端产品与执行来源、适配合同、产品前端、测试 runner 和当前后端核验器 SHA；测试明细同时绑定演练、目标计划、运行收据、双端来源与测试时间，并保存唯一 STOP source-generation 与派生 dataset-lineage 的完整文件描述；证明保存这两份文件的 SHA-256。已有结果不会覆盖；有失败、跳过、重试、缺场景、源码不匹配、明细不一致或数据验证尚未完成时不生成成功证明。最终 `restore verify` 必须同时收到三份相邻证据、原目标计划和 runner 根目录；维护 CLI 会从登记库构造权威上下文，由当前干净后端从 runtime launch 绑定的 registration 读取本侧参考计划，重新核验目标计划、运行中三进程、完整构建来源、runner 以及全部摘要，再登记成功。数据血缘必须与同一备份的 STOP、START、当前 source-runtime 全部一致；另一份独立 lineage 或另一侧参考计划不能替代此链。这一步仍会重新检查服务就绪和恢复时间，浏览器通过不等于最终恢复登记成功。最终核验使用统一 data 入口在编译时绑定当前后端 SHA；直接运行旧维护二进制会失败。解释器必须在工具文件配置中以已验证的绝对路径登记。核验器会以隔离模式重新执行 Python 环境检查，绑定解释器文件身份与摘要，限制输出和运行时间，并在前后再次核对解释器及双端 Git 来源。

### 参考环境的外部验收驱动

参考夹具环境计划、配置和预检收据守卫由正式绑定的 Rust 工具执行；Python 仍负责准备编排、secret set 轮换及 bootstrap 文件生命周期。seed close/rebind 的模型和证据守卫也由正式绑定的 Rust 工具执行；核验不授权远端写入或重放。 其余来源验证、已有数据核验、岗位批次、夹具运行与后继、监控预检及 DevEx 来源核验，仍由正式维护入口绑定当前协调器原生工具后启动各自私有程序。若报告工具绑定缺失、摘要变化或身份不一致，请保留阶段收据与原始失败，核对当前维护产物后重新进入正式命令；不要直接启动私有脚本、设置旧工具环境变量或尝试临时 Cargo 回退。绑定失败不会授权资源写入或未知结果重放。

完成同一 source_to_seed run 的复制、post-copy、seed 验证和 close 后，使用正式来源登记入口：

```powershell
cargo xtask check recovery clone seed-runtime --operation source-register --run-dir <absolute-run> --write
```

`<absolute-run>` 必须是当前后端 `.local-tests` 内已登记且无链接的运行目录。该操作必须显式传入 `--write`，不接受 `--request`、`--output` 或手工指定 attempt；它读取固定运行清单和阶段历史，由统一 Rust 阶段控制器分配 attempt 并直接执行来源登记，不启动 Python materialize 或登记脚本。执行期间实际持有目标 Generation 和复制账本 guard，要求完整历史生产者已停止，并进行两轮来源锚点、请求和 generation 验证。六项本地证据按固定顺序发布；失败保留已写前缀，不自动清理、重放或猜测多个旧目录。同一 run 已有其他成功来源登记时拒绝再次发布。正常收尾会核验并释放本次实际持有的控制锁，guard 文件可以继续保留；锁校验或释放失败时保留现场和异常残留锁，不手工删除后强行重试。

成功 stdout 的顶层 `status` 是 `stage_finished`，同时包含 `stage="seed-runtime"`、`mode="source-register"`、控制器分配的 `attempt`、内层 `result` 和 `restore_qualified=false`。内层 `result.status` 才是 `seed_source_registered`；`result.registration`、`result.source_request`、`result.source_storage`、`result.generation_verified` 均为含 `path`、`bytes`、`sha256` 的文件绑定，另外包含 `remote_writes=0`、`outbox_drained=true`、`restore_qualified=false`。后续调用应消费这些内层绑定，不能读取顶层 `registration`。`<run>/results/NNNN.json` 保存内层结果，NNNN 对应本次 attempt；六文件证据可续接唯一旧前缀目录，实际位置以返回绑定为准。私有 source request 保留在受控文件中，不把配置凭据复制到公开登记。登记成功不表示完整端到端恢复或正式演练已通过，也不授权远端写入。source-register 遇到 `HistoricalProducerCimUnavailable` 时会记录失败阶段并返回安全诊断，不返回成功的 `stage_finished` 结果。

`cargo xtask check recovery` 调用私有的本机 MySQL、mysqldump、原生 Rust S3 客户端和 Node 阶段程序准备演练数据与外部备份。参考计划只能通过 `inputs reference` 从已发布共享导出、对应侧 arm 和 fresh-target 完整前像推导，不能手工拼接源、工具或目标字段。入口固定来源和工具摘要、目标侧、fresh ownership 及独立工作目录；MySQL 使用计划内明确的客户端配置文件及其摘要；原生 S3 客户端直接读取计划绑定的受控明文配置，不启动 AWS CLI 子进程，也不通过环境变量传递访问密钥。凭据值不得进入计划或公开收据。工具不创建或扫描数据库，源目标均需事先初始化。 参考计划的源对象配置仅从绑定 runtime 的 `api_config_dir`、`api_configuration_sha256` 和 `APP_ENV=test` 推导；目标对象配置从 fresh 请求必需的 `config_dir` 及 `configuration_sha256` 推导。两侧公开配置只包含端点、region 和 `app_env/directory/files/sha256` 文件绑定；私有 `source.s3` 仅保存 `configuration` 文件绑定，不接受内联访问密钥或秘密密钥；源端点和 region 从已核验文件推导，并核对绑定属于实际 API 运行配置。目标配置仍核对 TOML 与 fresh 私有请求的连接字段完全一致，返回前复核配置、runtime 与 arm 文件，任何缺失或漂移均拒绝，不创建或修补配置。参考规模为 system 加十个普通租户、至少十万条实际岗位记录及至少 1 GiB 已登记上传对象，租户分布覆盖共享和独立目标；例如 256 个 4 MiB 对象。岗位记录属于控制库，租户业务表复制另由隔离业务 crate 生成资源验收覆盖。先用 `inputs reference --arm-input <side-arm-result> --fresh-target-verify <side-observation-directory/verify.json> --side base|candidate --id <backup-id> --work-dir <new-side-directory>` 预览参考计划，再追加 `--output <side-reference-plan.json> --write` 发布。

数据准备计划的 `dataset.request_interval_ms` 必须为 1000 至 5000 毫秒，限制每个固定客户端的实际 HTTP 请求启动频率；十一个租户可并发准备，每个租户使用自己的身份与地址。`dataset.timeout_seconds` 显式设置整阶段时限（1 至 604800 秒），例如参考规模预留 21600 秒；其他外部命令仍使用 1800 秒超时。收到 429 时保留失败，不通过重试或更换地址绕过限流。数据准备发生在备份与恢复开始之前，其耗时不计入恢复时间。

所有命令从对应干净后端工作树执行；`cargo xtask check recovery` 固定当前 `--backend-dir`，以下基于参考计划的阶段共用 `--plan <plan-json>`。公开入口在连接资源前完成严格参数解析：未知、重复、缺失或跨阶段参数均以参数错误拒绝。计划、输入、备份目录、工作目录和输出都必须位于当前后端 `.local-tests`，不得包含 `..`、经过链接或重解析点，也不能覆盖已有输出。外部数据库、对象存储和工具路径只从已经绑定的参考计划及收据读取，不能另传路径绕过 ownership。`plan` 的摘要、目标计划复核和完整输入预览，以及 `check-dataset`、`check-existing` 均为只读操作，不接受 `--write`。`inputs reference|product|bindings` 在 Rust 进程内只读推导并复核完整来源链，默认只在标准输出预览，只有同时给出新的 `--output` 和 `--write` 才发布证据。`dataset`、`backup`、`restore`、`copy`、`damage` 会创建会话、阶段证据或修改明确登记的隔离资源，必须显式传入 `--write`；该授权不放宽账本、前后像、ownership、幂等和未知结果核验。

随后用 `inputs product --reference-plan <side-reference-plan> --backup-receipt <same-backup.json> --comparison-sources <two-version-source-manifest> --arm-input <side-arm-result> --fresh-target-verify <side-observation-directory/verify.json> --side base|candidate --id <restore-id> --fault-at <utc-time>` 预览产品计划，追加 `--output <side-product-plan.json> --write` 发布。该入口从同一备份、比较来源、arm、fresh 目标和对应侧产品 SHA 推导原生 `RestorePlan`，并校验 `fault_at` 位于备份实际采集后 24 小时内；`--output` 与 `--write` 必须同时出现。参考计划的源侧只记录备份所需 API，目标侧另记录 API、Worker 和前端三个端点。

| 阶段 | 操作与输出 |
| --- | --- |
| `dataset` | 通过当前 API 建立套餐、十个租户、业务记录和关联对象，保存逐条创建证据与 `dataset/result.json`。 |
| `backup --inventory <original-shared-export-inventory-before.json> --source-generation <same-generation-stop-outer-result> --source-export-result <published-shared-export-result>` | 核对同一源代次的干净构建、当前业务复验、完整停止前后像与共享导出；精确导出数据和对象并核对摘要。`backup.json` 绑定共享导出身份、完整 source-generation 和 `backup/manifest.json`，随后用产品 `backup-register` 登记。库存直接使用共享导出的原始文件，备份 `id` 由计划派生，不能手工增补。 |
| `restore --target-plan <side-target-plan> --runtime-registration <side-runtime-registration> --backup-root <backup-directory> --record <restore-begin-record>` | 先复核全部输入与产品计划精确一致性，持运行控制锁重新核对完整源码、B0 重建及目标四库五桶完整前像。每次写前核验三端口空闲，完成后采集完整后像。分别保存 `restore-base.json` 或 `restore-candidate.json`，绑定原计划、备份、运行记录、运行登记和前后像；复制完成后仍需产品数据验证和真实业务证明。 |
| `copy --backup-root <backup-directory> --copy-id <independent-copy-id>` | 创建保留原摘要的独立备份副本；先登记副本并执行 `restore-begin`。 |
| `damage --backup-root <copy-directory> --artifact <manifest-path> [--missing]` | 仅损坏或删除指定副本产物，保留原备份；随后用 `restore-verify-data` 验证失败状态及告警。 |

正式双侧恢复分别使用 `target_side: base` 与 `target_side: candidate` 的参考计划和独立工作目录。通过 `cargo xtask check recovery plan --plan <side-reference-plan> --backup-receipt <same-backup.json> --comparison-sources <two-version-source-manifest> --arm-input <side-published-arm-result> --fresh-target-verify <side-observation-directory/verify.json> --product-plan <product-restore-plan-file>` 推导完整目标计划；加 `--output <new-target-plan-file> --write` 才发布。它把 base 固定映射到 b0、candidate 固定映射到 b1，并绑定同一共享导出、原备份摘要、本侧初始化与完整 ownership、探针、前端 SHA 和产品恢复计划。目标计划分别记录维护初始化的 `maintenance_execution` 与比较产品的 `product_execution`；双侧可共享候选维护工具，B0 产品运行仍必须使用已登记适配源码和对应构建。产品计划的 `fault_at` 必须使用 UTC `Z` 格式，小数位按实际精度保留 0、3 或 6 位，保证经过产品序列化后仍逐字段一致。`plan --plan <side-reference-plan> --target-plan <published-target-plan>` 重新核对全部绑定；未知字段、侧别混用、证据漂移或输出覆盖都会失败。

已有数据可以在原计划不变的前提下复验。先用 `cargo xtask check recovery check-existing --plan <original-plan-json> --side source` 核对源侧数据库 ownership 和已登记 API；随后运行 `cargo xtask check recovery dataset-prepare --plan <original-plan-json> --verify-existing <original-dataset/result.json> --side source --write`，读取原岗位样本并下载校验全部登记对象，将标准输出保存到新的独立证据文件。计划、预检和原数据收据必须是当前后端 `.local-tests` 内的绝对现有文件，入口拒绝链接、越界和隐式覆盖。`--write` 表示登录、注销会产生会话及审计副作用，业务记录和对象始终只读。省略 `--side` 时已有数据验证仍固定为 `target`；数据准备仍固定为 `source`，其他阶段不接受该选项。

环境 `prepare --output` 必须同时位于协调后端 `.local-tests` 内和已绑定执行后端的 `.local-tests/reference-fixture` 子目录；输出父目录须已存在，目标目录须不存在，且所有路径段不能经过链接。业务夹具由 `fixture --output-dir <coordinator>/.local-tests/<fixture>` 创建时，执行后端为 `<coordinator>/.local-tests/<fixture>/backend`，环境输出示例为 `<coordinator>/.local-tests/<fixture>/backend/.local-tests/reference-fixture/environment-seed-r1`。只位于协调后端的 `<coordinator>/.local-tests/environment-seed-r1` 会在创建任何输出文件前失败；诊断给出正确执行后端的绝对路径示例。

fresh 参考夹具环境准备会在源 bootstrap 输出目录独占创建私有 `administrator.toml`，内容严格为 `[administrator]` 下的 `tenant_id = "system"`、`username = "admin"` 和 `password`。当前密码由 Rust 随机生成并与 `bootstrap.admin_password` 不同；两份配置必须在生成参考计划前固定。公开 bootstrap 收据与参考计划的 `dataset.administrator_credentials` 只保存该文件的 `path`、`bytes` 和 `sha256`，不包含明文。初始化密码仅用于 fresh 系统管理员及十个新租户 owner 的受限登录；每个账号先核对受限 context，落盘独占 intent，再发一次改密请求，记录确认后丢弃旧会话，使用显式当前密码重新登录并验证同一主体、首次改密状态和必要权限，最后发布激活完成收据。十一账号只在这个隔离夹具内共用该当前密码。

改密超时、响应中断、服务端错误、确认落盘失败或遗留 intent 都必须保留现场并停止，不能重新发送改密、尝试旧密码、清除数据库标志或改写凭据文件补签。已有数据的 target/source 核验及 source/clone 原生复验只读取绑定的当前凭据；遇受限会话直接失败，绝不自动改密。

验证结果记录实际侧、scope、原计划及数据收据摘要和只读动作范围，状态为 `existing_data_verified`，不表示恢复成功，也不单独证明源码干净。源侧复验结果不能用于目标恢复证明。

正式备份源按[数据文档](data.md#备份恢复与保留)在原复制账本完成 source-generation start，再执行 `cargo xtask check recovery source verify --source-generation <same-generation-start-outer-result> --output <same-generation-gNNNN/verification/source-runtime.json> --write`。构建、原数据集血缘、当前 scope、API、前端 Origin 与 pacing 均由已绑定的 start 和 lineage 推导，不接受另一份计划、构建或旧 dataset 作为替代输入。复验由 Rust 从产品执行 commit 的 Git blob 创建同代不可覆盖的契约 staging，并绑定协调器来源与唯一原生工具摘要；不读取可变 checkout 中的契约，也不接受继承的 Git 仓库或 replace 覆盖。原生生产者预载 OpenAPI、配置绑定和血缘并发布 ready 摘要，协调器在服务授权前核对完整文件集合及摘要。staging 创建中断时保留部分文件，重复创建和并发创建均不能覆盖；须先核对失败证据。来源物理绑定和数据血缘由同一个原生 Rust 工具核验：显式配置目录中的明文 `password` 必须与冻结 MySQL 客户端文件一致，公开绑定不含凭据；配置、祖先收据、复制计划和完整像发生变化时立即拒绝。逻辑目标必须精确覆盖，源目标按服务器身份与数据库名区分，不能将同名表或另一物理库当作同一来源；历史 C52 计划只接受已审计的精确文件和内容摘要。数据库诊断在 Rust 中按冻结凭据脱敏，配置写入仅创建新的 `app.test.toml`，失败保留现场且不自动重放。十一登录主体、33 个授权缓存键及完整前后像的允许写入范围由 Rust 精确校验。业务阶段只使用内存中的预载契约，因此授权窗口修改后再恢复 staging 也不能改变实际请求。复验同时核对 API/Worker 的摘要、配置、创建身份、监听端口和就绪状态，并披露登录注销的精确行级副作用。

来源验收证据核验只接受授权前已登记文件及合同内的有限后续文件；数据库诊断与 stdout 必须一一绑定，内核运行计数可以变化但进程创建身份不能变化。来源、文件集合或摘要漂移均保留失败现场。恢复缓存清理时，只有完整冻结计划仍有效且全部 33 个键已消失，才可将未知结果登记为已核对，删除数保持空值；混合状态不得重放清理。

成功后在同一账本执行 source-generation-stop，显式消费该 `source-runtime.json`。停止阶段亲自回收已登记 API/Worker 树，核对完整前后像并记录停止观察时间，然后才允许唯一 source-export。给 `backup` 传入同一停止外层结果、共享导出外层结果及其原始库存；备份前后均验证源没有换代，以及当前复验、实际停止观察、正式库存的时间顺序。清单先采集、进程后来才停止的流程会失败。验证或停止失败均保留证据且不可重放；不改写历史计划、收据、摘要或完成时间。

B0/B1 正式对照前，使用 `cargo xtask check recovery source comparison-capture --help` 查看来源参数，再以 `comparison-capture ... --source-export-result <unique-export-outer-result> --output <new-source-manifest> --write` 生成严格的双版本来源清单。B0 必须分别提供原产品源码、已登记的工具适配源码和前端源码；B1 必须提供最终干净的双端源码。两侧都必须提供对应源码上的 v2 后端与 v3 前端生产构建收据，并共同绑定同一个已发布 `source-export` 的外层文件摘要及内部导出身份。`cargo xtask check recovery source comparison-verify --receipt <source-manifest>` 只读重建并核对全部来源、构建产物、完整前端 `dist` 和导出证据；它不创建恢复 run、锁或业务写入，也不接受旧格式字段。历史 B0 前端尚无当前 TOML 和受管身份布局，必须另行完成正式适配登记；当前明确拒绝旧布局，不代表 B0 恢复已经验证通过。

B0 后端构建仍由当前受信任工具执行，但 `--source-backend` 必须指向登记的适配提交，并同时提供 `--expected-head <adapter-sha> --adapter-contract legacy-stable-readiness-b0-v1 --product-backend <original-b0-absolute-worktree>`。入口会从当前工具内嵌补丁重建适配树，核对原 B0、适配提交、补丁摘要和产品输入域，任一项不匹配都会在调用 Cargo 前失败；不能复制当前脚本到旧工作树或手工补写 v2 收据。

恢复浏览器套件逐一登录十一个租户读取备份前已有的岗位，下载并校验全部原有上传对象，再执行正常核心业务。故障与 Worker 重启场景继续由普通全栈套件执行。每阶段保留独立收据，失败不覆盖、不自动清理资源；只有最终产品 CLI 核算后的成功记录能证明 24 小时恢复点与 60 分钟恢复时间。

### 指标与告警

API 与 Worker 的备份采集器每 60 秒读取控制库汇总，整次采集最多等待 10 秒，输出到现有 Prometheus 指标接口。运行时监控页显示采集状态、必需及异常资源、最近恢复结果与耗时；未知、失败和陈旧状态不表示当前数据可用，保留的有效快照仅供诊断。备份故障独立展示和告警，不改变 API readiness。

`ryframe_backup_last_success_timestamp_seconds` 取所有必需目标的最旧有效备份采集时间，重新登记旧备份不会刷新恢复点。`ryframe_backup_resources` 使用固定的 required/missing/expired/invalid 维度，`ryframe_backup_collector_up` 表示采集成功与否，`ryframe_backup_collector_last_success_timestamp_seconds` 记录最近成功读取汇总的时间；后者缺失或超过 150 秒会触发采集告警，不能用它替代备份恢复点。恢复指标包括最新完成结果、耗时、恢复点年龄和 running/overdue 汇总。指标没有租户、端点或对象键标签。

`deploy/prometheus/ryframe-alerts.yml` 在 23 小时预警、24 小时告警，并分别检查缺失、过期、校验失败和采集失败。收到恢复失败或超时告警时先查看登记状态和保留的演练证据，不通过改登记时间消除告警。

将该规则文件只读挂载或复制到现有 Prometheus 的 `/etc/prometheus/rules/ryframe-alerts.yml`，在其配置中显式加载这个文件。下例中的两个 HTTPS 地址应替换为现有监控入口，分别转发到 API 的 `/api/v1/monitor/metrics` 和 Worker 健康端口的 `/metrics`；生产 Compose 中的 Worker 默认只在后端网络可见，需要由现有监控网络或反向代理提供访问。密钥文件只保存与 RyFrame 明文配置 `monitor.metrics_bearer_token` 相同的 token 内容；这是 Prometheus 自身的凭据文件读取，路径以 Prometheus 所在主机或容器为准。

完整抓取与规则加载配置见 [Prometheus 配置示例](../deploy/prometheus/prometheus.example.yml)。
沿用现有 `alerting.alertmanagers` 配置，并将 `service=ryframe` 的 warning/critical 告警路由到运维通知渠道。先执行 `promtool check config /etc/prometheus/prometheus.yml` 与 `promtool check rules /etc/prometheus/rules/ryframe-alerts.yml`，通过后按现有平台的配置重载方式生效；Prometheus 支持 `SIGHUP`，也支持在启用生命周期接口后通过 `POST /-/reload` 重载。[配置与重载方式](https://prometheus.io/docs/prometheus/latest/configuration/configuration/)

重载后在 Targets 中确认两个抓取目标均为 UP，在 Rules 中确认 `ryframe.application` 规则组加载且健康，并查询 `ryframe_backup_collector_up`、`ryframe_backup_resources` 和 `ryframe_backup_last_success_timestamp_seconds` 检查实际采集结果。API 与 Worker 各暴露一份部署汇总，按 Prometheus 的 `job`、`instance` 区分采集进程，不能将两份计数相加。最后通过隔离演练产生真实失败状态，核对告警进入 Alertmanager 并送达通知渠道；仅通过配置检查不作为通知送达证据。正式恢复的隔离演练统一使用 `cargo xtask check recovery monitoring`：先创建只含权限已收紧 `metrics-token.txt` 的新 run 目录，以 `bind --help` 核对参数，再用 `bind ... --write` 绑定同一次正式恢复的 `runtime.json`、目标计划、唯一 run ID、Prometheus/promtool 3.5.0、Alertmanager/amtool 0.34.0、四个工具绝对路径及三个互异的非特权回环端口，生成不可覆盖的 `binding.json` 和当前干净提交的不可变 staging；token 必须与运行中的 API、Worker 指标凭据一致，内容不得进入命令行或收据。监控绑定、输入 staging、回环 HTTP、webhook、Prometheus/Alertmanager 进程树、观测和关闭由 Rust xtask 执行；`status` 与 `result` 原生读取阶段证据，不启动 Python。`bind|start|observe|close` 仍通过固定只读协议复用现有 Python 核验器，主动检查正式恢复的静态来源和三端存活代次，不能用 JSON 字段匹配代替。绑定同时记录当前 xtask 可执行文件摘要；阶段执行期间保留凭据、工具、规则和协调程序的文件句柄，并在结束时复核身份与摘要。Windows 凭据要求受保护的显式 DACL，仅允许当前用户、System 和 Administrators；Unix 要求当前用户拥有且其他用户无访问权限。权限无法核验时失败关闭，不自动放宽。后续只对该绑定依次执行只读的 `status --binding <absolute-path>`，以及显式写入的 `start|observe|close|result --binding <absolute-path> --write`，每次只执行 `status` 返回的 `next_action`，失败、中断、未知发布结果或 `*-failure.json` 必须保留并转入其指示的关闭或人工核对，禁止删除证据后重放。start 管理回环 webhook、Alertmanager、Prometheus 三棵进程树；observe 核对 API/Worker bearer 拒绝与成功响应、targets、固定规则、精确 firing/resolved probe 及 promtool 23/24 小时边界，receiver 只允许 `ryframe-local-webhook` 且外部联系数必须为零；close 等待全部后代退出和端口释放；result 只在 start、observation、close 均成功后排他发布并重验全部摘要，遗留 pending 时先核对文件身份和内容，不能用重跑猜测结果。

## 发布门禁排障

协调版本 tag 同时触发双方日常 CI 和 Extended CI。后端的 core 与业务 crate 全栈任务均检出配套前端 tag，产物内记录双方精确 SHA、run ID 和 attempt；业务 crate 产物另附从干净源码生成隔离工作树的收据。Release 通过 `cargo xtask check release source` 核对双端 tag、版本与契约来源，通过 `cargo xtask check release ci` 在校验阶段和实际创建 Release 前分别检查四组最新运行、两套全栈产物及必需 job；缺失、失败、取消、超时、错误 SHA、跳过必需 job 或最新重跑未成功都会阻断发布。两个入口直接由 Rust xtask 执行，不依赖 Python，并支持 `--plan` 只查看实际任务图。

Release 在检出配套前端前通过 Rust `frontend-source` 从工具文件或标准 GitHub owner 身份选择仓库，配置方式见[开发指南](development.md)。校验阶段冻结该仓库、双方精确提交及 tag 对象，发布阶段复用冻结结果，不重新读取仓库选择。前端仍按显式发布 tag 检出，仓库配置不会改变双方 tag 必须匹配及精确来源核验。

检查 Release 保存的 `release-ci-evidence.json` 与 `release-final-evidence.json`，按其中 run ID 和 attempt 定位失败。默认总等待上限为 5400 秒，GitHub API 分页和单次调用共享截止时间。修复后对同一目标源码重跑相应 CI，再重新运行 Release；不移动已有 tag，也不以其他提交的成功结果替代。发布资产仍为 GitHub 源码归档。

## TLS 证书

检查到期时间、主机名、完整链和私钥权限。轮换后验证 MySQL、Redis、对象存储、OTLP 与 Nginx；不要关闭证书校验作为长期修复。

Redis 一般协议、Redis TLS、S3 HTTPS 和 OTLP HTTPS 真实测试分别使用后端根目录下 `.local-tests/service-tests/{redis,redis-tls,s3-https,otlp-https}/config` 中的显式 `AppConfig` 测试配置。公共 `app.toml` 可缺省，`app.test.toml` 必须存在；仅目录缺失会明确跳过并标记真实链路未验收，已有目录缺文件、配置非法、无权限或任何路径层级含链接/重解析点均失败。Redis 必须配置 `mode="required"`，TLS 角色还须 `tls=true`；连接、密码、DB、池和超时来自文件。每次测试在独占运行目录派生 scope 快照，保留其余字段及公共文件字节，清理结束前持有快照；来源漂移拒绝重新捕获。S3/OTLP 的 `test.toml` 必须且仅含 `ca_certificate` 绝对文件路径，客户端显式加载该 CA，不用环境变量传递项目端点、凭据或 CA，也不降级 HTTPS。

TLS 执行器消费调用方已从 `.local-tests/integration/ci.toml` 加载的同一 `IntegrationConfiguration` 对象，不重新加载参数，也不使用环境回退。文件严格要求 `format_version=1`、`run_id`、`artifact_root`、`redis_config_dir`、`openssl_program`，仅 `test_jobs`、`timeout_secs` 可省略。`run_id` 为 1～64 位 ASCII 字母、数字或短横线；`test_jobs` 默认 4、范围 1～64，Windows 必须为 4；`timeout_secs` 默认 900、范围 1～3600 秒。相对路径以后端根目录解析；路径不得为空、含控制字符或父目录跳转，各层拒绝链接/重解析点。`openssl_program` 指向普通程序文件，明确值 `"openssl"` 则使用工具 PATH。上游 Redis 必须显式配置回环主机和端口，并使用普通 TCP；配置 `tls=true` 会明确报告不支持，不自动降级。

fixture 启动后，执行器独占创建三个 TLS 角色配置目录。Redis 保留捕获的账号、DB、客户端证书/私钥、池和超时，只替换代理端点与 CA；S3/OTLP 使用 fixture 端点及测试字段。执行前后复核输入和发布文件；已有目录不覆盖，发布不完整、来源漂移或外来文件均保留现场及证书并报错。该执行器和测试消费者的文件协议不能替代 CI 生产者接线；工作流仍须生成这些文件并同步产物上传路径，真实服务验收须另行执行。

## 故障记录

隔离参考夹具服务通过 `cargo xtask check recovery fixture services status --review <review-receipt> --environment <bootstrap.json>` 查看只读状态，输出原账本、控制器身份及下一合法操作。关闭时改用 `close` 并显式追加 `--write`，按 Redis、RustFS 顺序回收原进程树，确认端口释放；数据库、对象及服务目录保留。正常关闭后需要继续同一夹具时，先确认 `status` 返回 `restart`，再把状态中的当前 `state.json` 路径传给 `restart --owner-binding <state.json> --write`；重启追加新代次，不重放关闭或先前业务写入。若宿主在正常关闭收据发布前回收了整棵服务树，`status` 仅在全部登记身份消失、端口关闭且 ownership 未变化时报告外部终止，并返回当前 `state.json` 的 owner binding；先用该文件执行 `recover --owner-binding <state.json> --write` 追加独立核对证据，再对新的 state binding 执行 `restart --owner-binding <state.json> --write`。重启在新代次目录保存请求、进程树和前代 lineage，不补造旧代次的正常关闭证明。业务 crate 产品运行时启动后，同一恢复入口先发布只含来源、工具、端点、限流和凭据变量名的浏览器绑定，再明确执行前端 `build --real` 与 业务 crate preview 浏览器阶段；绑定和预检不创建登录预算账本，实际登录才首次创建，失败或中断后保留 intent 且禁止重放。Corepack、Node、Vite、Playwright 和 Chrome 都处于本次监督进程树内，控制器退出前核对完整后代停止证明；日志及收据不保存密码。业务套件内的 Worker 控制由正式全栈监督器启动并登记进程树；套件进程树收口会连同其中的 Worker 一起回收，Worker 控制仅在监督进程、成员监督器与产品进程全部确认退出、端口释放且不存在正常关闭证明时登记外部终止，证据阶段随后在套件进程树之外重新启动 Worker；任一身份存活、PID 复用或记录不一致时失败关闭，不补造正常关闭证明。新首代 RustFS 在产品代码执行前建立 Job Object 或 Unix session；独立的私有成员监督器持有完整成员的内核句柄，确认全部退出后才签发完成证明。历史收据缺少进程树归属时不能补造完整关闭证据。

死亡控制器通过同一入口的 `recover --review <review-receipt> --environment <bootstrap.json> --owner-binding <owner-file-binding-json-from-status> --write` 显式恢复。它只清理已核对的本地控制锁并追加账本，不停止服务或重放未收尾操作；PID 已复用、来源改变、未知启动或关闭结果均须保留现场核对完整前后像。已关闭代次不能再签发 fresh-target 请求；已登记目标只接受严格核验过的已收尾生命周期追加。

记录时间线、影响租户、请求或任务 ID、根因、处置和验证结果。配置字段以配置结构为准，指标和告警以部署资产为准，不在本文维护重复清单。

监控绑定中的 `environment` 保留为正式启动私有配置文件的描述符，文件内容必须直接写明文 `metrics_bearer_token`、`scope_id`、`api_config_dir` 和 `worker_config_dir`，例如：

```json
{
  "metrics_bearer_token": "<plaintext metrics token matching the running API and Worker>",
  "scope_id": "<scope of this formal recovery>",
  "api_config_dir": "D:/workspace/backend/.local-tests/restore/api-config",
  "worker_config_dir": "D:/workspace/backend/.local-tests/restore/worker-config"
}
```

该文件只接受这四个字符串字段；旧环境变量键、旧 `environment` 包装、额外字段、非字符串、scope 或令牌不匹配均被拒绝，不读取父进程环境作为回退。文件路径、摘要和内容持续绑定原正式启动来源，不能仅为监控另建或替换该文件。Prometheus 继续通过受保护的 `metrics-token.txt` 和 `credentials_file` 读取同一令牌；scope、角色及运行路径通过固定参数或 supervisor 标准输入协议传递。所有监控子进程清空项目与凭据环境，仅白名单保留操作系统变量和固定的回环代理设置；预检 stdin 的受管文件位于后端 `.local-tests`，退出后自动删除，预检错误不透传子进程原始诊断。

API 与 Worker 各自通过 `--config-dir` 接收绝对配置目录，并使用 `APP_ENV=test` 选择必需的 `app.test.toml`；可选公共 `app.toml` 先加载。文件中的 scope、指标令牌必须与私有文档一致，API、Worker 端点必须匹配登记值，任务模式必须为 external；两侧 `snowflake_worker_id` 必须有效且互不相同。启动收据绑定目录路径和配置文件摘要，不复制凭据；后续文件修改、增加或删除均导致核验失败。恢复运行子进程只继承必要 OS 变量，后端角色另显式设置 `APP_ENV`。Rust 协调器将严格控制请求写入后端 `.local-tests` 的受管文件，保留至 Python 阶段退出并检查删除；stdin 继续用于现有权威上下文。旧环境控制请求与按角色覆盖的项目环境字段均不再读取。 恢复生命周期的代次状态、首次目录 intent 和启动收据由绑定 Rust 工具处理；首次 intent 或成功收据已存在、文件集合异常或冻结前像不符时保留现场，先通过现有 status/recover 核对，不覆盖结果或自动重放。
