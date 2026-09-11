# 运维

## 部署与发布

生产镜像使用固定摘要的 Distroless Debian 13 运行层，只包含 API、迁移、Worker 及其运行依赖，不带 shell、包管理器、`curl`、生成器、文件维护或 reset。API 与 Worker 由自身的 `--healthcheck` 命令探测本机 `/readyz`。部署前复制 `deploy/.env.production.example`，在受控环境中填写镜像摘要、服务地址、`APP_SCOPE_ID` 和各密钥文件路径。

先展开 Compose 配置检查最终变量和挂载，再启动服务：

```powershell
docker compose --env-file deploy/.env.production -f deploy/compose.prod.yml config
docker compose --env-file deploy/.env.production -f deploy/compose.prod.yml up -d
```

Compose 会先更新控制库与租户库，再启动 Worker 和 API。启动后检查 API `/livez`、`/readyz`，以及 Worker 健康端口的 `/readyz`。Nginx 只代理 API 的 edge 网络端口，MySQL、Redis 和对象存储保持在内部网络。

当前版本只接受全新生产库。已有真实生产旧库需要先设计并演练非破坏升级方案，不能使用 reset 代替升级。

## 非生产重建

`ryframe-reset` 只在显式 `bin-reset` feature 下编译，生产镜像不复制该二进制。执行前必须停止 API、Worker 和 scheduler，并完成全部只读预检。

```powershell
$env:APP_ENV = "test"
cargo xtask data reset plan
cargo xtask data reset execute --plan-hash <sha256> --confirm-reset <精确短语>
```

执行顺序固定为对象前缀、Redis namespace、物理数据库、控制 baseline、租户 baseline、验证和锁释放。清单、ledger 和 report 不包含秘密；普通阶段失败且锁释放成功时，只允许使用同一清单和 ledger 续跑。生产环境在读取配置或访问外部资源前永久拒绝。

报告的 `status` 区分 `completed`（本次完成）、`reused`（复用同一 ledger 的原完成）和 `failed`。`completed_at` 记录锁释放后持久化的原完成时间，重复执行不更新该值，也不重新运行预检或破坏性阶段；`reported_at` 仅表示报告生成时间。锁释放失败、中断或缺少完成证据时，重复执行仍失败，必须人工核对服务停止状态、MySQL 环境锁及清单内精确资源，保留原始失败记录。

新的绝对 `RYFRAME_RESET_STATE_DIR` 表示一次新的维护操作，必须重新生成并核验 plan、ownership 和资源范围；不得通过删除或覆盖旧 ledger 触发重置。当前开发版本使用 ledger v4 和 report v2，旧版账本明确拒绝，不自动转换，旧本地证据继续保留。

## HTTP 5xx 与高延迟

先按请求 ID 检查 API 日志和 trace，再检查数据库、Redis、对象存储与 Worker。确认是单一依赖故障后按降级策略处理；不要通过扩大重试放大故障。

## Redis 降级

检查连接模式、TLS、ownership marker 和 scope 前缀。授权缓存不可用时必须失败关闭或按配置禁用缓存；禁止清空共享 Redis DB。

## Redis 与对象存储约定

- Redis `timeout_secs` 统一约束连接、响应和事务，范围 1–60 秒；兼容 S3 后端使用 `object_storage.request_timeout_secs`，范围 1–300 秒。不得在调用点叠加无上限重试。
- 就绪探针只执行 Redis `PING` 和对象存储私有桶检查，并受独立短超时保护；`required` 依赖失败才阻断就绪，可选依赖明确进入降级状态。
- `ryframe_connector_operations_total` 与 `ryframe_connector_operation_duration_seconds` 只使用 connector、固定 operation 和 result 三类低基数标签。端点、租户、Redis 键、桶名和对象键禁止进入指标标签。
- Redis 与 S3 配置的 `Debug` 输出会隐藏密码、访问密钥和客户端私钥。S3 非成功响应只保留稳定操作名和 HTTP 状态，直接丢弃远端响应体；传输错误移除请求 URL。文件维护错误只携带内部文件 ID，不记录桶名、对象键、Authorization、签名或负载。
- 确定性测试使用本地实现或离线协议样本；真实协议测试必须显式启用，并使用唯一 schema/namespace 与精确清理，禁止 `FLUSH`、`KEYS` 和模糊对象前缀删除。

## Refresh Token 重放

立即撤销 token family 和当前会话，核对用户授权版本、客户端 IP 与审计事件。确认泄露后轮换相关凭据并保留证据。

## 限流拒绝

区分租户配额、账号策略和基础设施故障。只在确认业务容量允许后调整配置，不删除限流键或绕过租户边界。

## 数据库副本与读回退

检查复制延迟、连接健康和 read consistency。授权、任务、导出与状态变更必须回到主库；不要把强一致读临时改为副本读。

## 后台任务

检查 lease、heartbeat、attempt、公开错误和死信状态。数据库会拒绝重复领取；修复原因后使用受控重试，不直接改任务终态。

## 定时调度

核对表达式、时区、misfire 策略和下一次运行时间。避免同一业务任务由多个 scheduler 重复创建。

## 消息中心投递

检查 outbox、分发任务、目标受众和多语言参数。先恢复幂等消费，再处理积压；不要跳过租户或权限过滤。

## OpenTelemetry 导出器

导出失败不应阻断业务请求。检查 endpoint、TLS、超时和队列丢弃指标，恢复后确认 traceparent 继续传播。

## 数据库连接容量

按控制库、租户目标和 Worker 分别统计池使用量。优先处理泄漏、慢查询和错误并发，再调整池上限；总连接预算不得超过数据库限制。

## 磁盘容量

检查 `target`、日志、临时 XLSX、上传暂存和本地对象目录。只删除已确认的构建缓存或已过期产物，保留最新调试符号和 reset ledger。

## 备份失败或过期

停止依赖无效备份的迁移操作，检查目标、校验和、采集时间和保留策略。恢复后重新生成并验证备份，不手工标记为成功。外部工具负责备份与还原字节，RyFrame 只负责清单采集、登记、恢复验证、状态和告警。

### 参考环境与一致性

参考演练采用系统租户和 10 个普通租户，覆盖共享与独立数据目标，至少 10 万条代表性业务记录和 1 GiB 对象数据。外部运维调度每 12 小时执行完整备份，保留至少 7 天。演练报告必须记录 Windows 机器、MySQL/RustFS 版本、存储和网络条件；24 小时恢复点、60 分钟恢复时间是该规模下需要实际验证的目标。

开始采集前暂停业务写入，停止 API 写入入口、Worker、scheduler 和消息等生产者。在整个清单采集、数据库转储和对象复制期间保持静止，避免把多个时间点的数据登记为一致备份。数据库按完整主键顺序计算摘要，对象采用流式 SHA-256；清单包含源码 SHA、schema 指纹、物理数据库身份、租户 placement、对象前缀、字节数和摘要，不包含密钥。

### 采集和登记

在 Windows 维护机通过 `cargo xtask data` 执行备份和恢复登记；配置和密钥放在受控的忽略目录。该维护能力不包含在生产运行镜像中。

```powershell
cargo xtask data --help
cargo xtask data backup inventory --output .local-tests/backup/inventory.json --source-sha <精确后端SHA> --quiesced-at <暂停写入的RFC3339时间>
```

输出文件必须事先不存在。随后由外部工具按清单中的精确数据库、表和对象前缀完成复制与校验，再补齐 `id`、`completed_at`、`retention_until` 和 `artifacts`，形成 `BackupManifest`。每个 `db:<逻辑目标键>` 与 `objects:<桶>` 至少对应一个文件产物，登记其 `relative_path`、`bytes`、`sha256`；相对路径不得越出备份根目录。结构定义见 `crates/ryframe-application/src/ports/backup/manifest.rs`。

需要检查尚未分配租户的目标或初始化前后状态时，使用 `cargo xtask data target inventory --target <已配置目标key> --output <新文件>`。该命令只连接明确配置的数据库，校验 schema 和精确全表目录，并在只读事务中计算完整行摘要；`target.database` 保存业务表与 placement，`target.preserved_tables` 单独保存 ownership、迁移账本和备份恢复登记表。它不访问对象存储、不修改目标，也不把未使用的目标遗漏为“空清单”。控制库与独立目标之间的一致性仍要求外部停止所有生产者；单份目标清单不表示备份或恢复成功。

```powershell
cargo xtask data backup register --manifest .local-tests/backup/manifest.json --backup-root .local-tests/backup/files
cargo xtask data backup status
```

登记会重新读取文件验证大小及 SHA-256，缺失或损坏会登记为无效并返回失败。同一备份 ID 只允许重复验证相同清单，不允许替换数据。登记成功表示已校验备份文件，尚不能表示恢复成功。状态查询只读取控制库，不依赖对象存储当前是否可连接。

### 隔离恢复与验收

1. 准备全新的隔离数据库和对象前缀，先按当前基线初始化并建立目标 ownership。逻辑 target key 保持一致，物理数据库、scope、JWT 密钥和 Redis namespace 必须与原环境不同，Worker 使用 external 模式。维护机的 `APP_CONFIG_DIR` 指向登记库，`--restore-config-dir` 指向隔离环境配置；检查实际生效的 `APP_*` 覆盖值，避免把源配置覆盖到目标上。
2. 按 `crates/ryframe-application/src/ports/backup/restore.rs` 的 `RestorePlan` 填写完整目标、故障时间、前端 SHA、API/Worker 就绪地址和新对象前缀。物理数据库身份必须匹配，不能指向任一原业务库。
3. 在开始外部还原之前执行 `restore-begin`。重复相同 plan 不重置计时。外部还原仅覆盖该 plan 中已核验的隔离资源，保留目标自己的 ownership 和备份登记元数据；数据库转储只含清单中的业务表，不执行指向源库的 `USE`、数据库创建或跨库语句。对象路径仅替换 scope 前缀，不能复制源 ownership 标记。
4. 停止目标业务写入，执行 `restore-verify-data`。它重新验证备份文件、当前 schema、完整表内容、租户关系和完整对象集合，缺失、多余、损坏或错误 ownership 都会失败。
5. 使用全新 Redis 临时状态启动 API、Worker 和前端，完成真实浏览器业务验收，再提交绑定演练 ID、plan hash、源码 SHA、scope 和时间范围的 `RestoreBusinessProof`。`restore-verify` 再检查两项就绪探针，只有数据与业务验证均通过且总计时不超过 60 分钟、备份恢复点距离故障不超过 24 小时才记录成功。

```powershell
cargo xtask data restore begin --plan .local-tests/restore/plan.json --restore-config-dir .local-tests/restore/config
cargo xtask data restore verify-data --id <演练ID> --backup-root .local-tests/backup/files --restore-config-dir .local-tests/restore/config
cargo xtask data restore verify --id <演练ID> --proof <runner>/.local-tests/playwright-real/restore-<演练ID>.json --tests-receipt <runner>/.local-tests/playwright-real/restore-<演练ID>-tests.json --runtime-receipt <runner>/.local-tests/playwright-real/restore-<演练ID>-runtime.json --target-plan .local-tests/restore/target-plan.json --runner-root <干净测试runner工作树> --restore-config-dir .local-tests/restore/config
```

首次向 fresh target 写入数据库或对象前，执行 `cargo xtask check recovery runtime register --plan <参考计划JSON> --target-plan <目标计划JSON> --output <新runtime-registration.json> --write`，登记目标从未启动。登记入口在 ownership 控制锁内，写入前后核验目标运行目录没有 lifecycle、launch、进程树或未知文件，并证明 API、Worker、前端三个精确端口空闲；缺少进程收据本身不能作为停止证明。正式恢复执行器使用同一登记锁包住完整写入过程，并在每次数据库或对象写入前及退出时重新核验。登记文件绑定两个计划的绝对路径、大小和 SHA-256 及零进程观察；恢复收据绑定该登记文件，计划或运行现场变化后必须新建登记，不能覆盖旧文件。

一次完整验收需保留成功恢复和损坏或缺失备份失败演练的原始日志、校验结果、trace、截图和视频。失败不会被登记覆盖为成功，也不会自动操作原业务资源。`running`、`data_verified`、`succeeded`、`failed` 分别表示已开始、数据通过、全部通过与失败；超过时限且未完成的演练仍会触发告警。

### 生成恢复业务证明

把 `restore-verify-data` 成功返回的完整记录放入绑定文件的 `record`，原备份清单放入 `manifest`，参考数据准备收据的 SHA-256 放入 `dataset_sha256`，保存到忽略目录。前后端必须是清单绑定 SHA 的干净源码，使用 `cargo xtask check recovery runtime build --source-backend <后端绝对工作树> --expected-head <后端完整 SHA> --source-frontend <前端绝对工作树> --expected-frontend-head <前端完整 SHA> --output <后端工作树/.local-tests/restore/build.json> --write --frontend-dir <当前工具前端>` 统一生成并核验实际 API、Worker 和前端生产构建收据。历史源码不需要提供当前 CLI；协调器使用当前干净工具调用目标工作树自己的 Vite，并在目标前端写入 `.vite/restore-build.json`。已有前端收据会经过相同的严格核验后复用，绝不覆盖或补造构建事实。两份当前构建收据均使用 v2 格式，分开绑定按产物角色计算的产品输入、验收工具输入、完整源码清单、实际工具链和有效构建参数；前端另绑定 production 模式读取的环境文件及完整 `dist` 文件清单。旧格式仅保留为历史文件，不能用于当前恢复。

`runtime start` 必须同时传入 runtime registration、完整 target plan、双端来源、构建收据和数据绑定；它从 target plan 绑定的环境文件启动 API、Worker、前端，并为每个角色先保存 operation intent，再用同一 Job Object 或进程组登记完整树。首次创建运行目录前还会在目录外写入唯一 creation intent，因此控制器在目录创建、状态写入或进程收据发布的任一切点退出后，都只能通过绑定 owner 和 generation 的 `runtime recover --write` 接管。每个 generation 只允许当前 operation ID 对应的日志、进程、完整树、成员、控制、结果、完成和 launch 收据；未知文件、目录或链接会阻断继续操作并保留现场。`runtime status` 只读派生真实的 `registered_not_started`、`running`、`degraded`、`exited`、`interrupted` 或停止状态；`runtime stop --generation <N> --write` 反向回收三棵树并证明端口释放。
三端就绪后，用 `cargo xtask check recovery runtime bind --source-backend <执行后端工作树> --source-frontend <前端工作树> --build-receipt <build.json> --launch-receipt <generation目录/runtime-launch.json> --bindings <绑定文件> --output .local-tests/restore/runtime.json --write` 签发 v3 运行收据。普通候选的产品与执行后端必须是同一提交；B0 另外显式传入 `--adapter-contract legacy-stable-readiness-b0-v1 --product-backend <815c5eaf 产品工作树>`，执行来源必须是已登记的 `c05114bc` 适配提交。运行收据分别保存备份来源、后端产品来源、后端执行来源、适配合同、前端来源以及三进程 generation，不能用备份清单的源码 SHA 冒充运行二进制来源。该入口核对源码、二进制、进程创建身份、完整树、探针监听端口及实际返回的前端文件，配置目录正确或 HTTP 200 均不能单独充当来源证明。

前端成组设置 `RYFRAME_RESTORE_BINDINGS`、`RYFRAME_RESTORE_RUNTIME_RECEIPT`、`RYFRAME_RESTORE_TARGET_PLAN`、`RYFRAME_RESTORE_BACKEND_DIR`、`RYFRAME_RESTORE_RUNNER_SHA` 和 `RYFRAME_RESTORE_VERIFIER_SHA`：前三项分别指向绑定文件、运行收据和严格目标计划，后三项指向当前干净协调器源码、当前干净前端测试 runner 的完整 SHA，以及当前干净后端核验器的完整 SHA。另用 `RYFRAME_E2E_SCOPE_ID`、`RYFRAME_E2E_BASE_URL` 和 `RYFRAME_RESTORE_DATASET_RECEIPT` 绑定新 scope、已启动的生产站点和派生数据血缘收据，再运行 `corepack pnpm check --stage browser --real --server preview`。目标计划和运行收据独立绑定实际产品前后端，因此 B0 产品前端可以与当前测试 runner 不同。测试开始及结束均复核来源；运行期间替换进程、修改源码或产物都会拒绝生成成功证明。各文件只保存摘要和资源归属，不包含密钥。

测试覆盖全部恢复核心场景并通过控制台、网络和 axe 断言后，结果写入 runner 的 `.local-tests/playwright-real/restore-<演练ID>.json`，测试明细和运行收据副本使用同一前缀相邻保存。证明明确记录备份来源、后端产品与执行来源、适配合同、产品前端、测试 runner 和当前后端核验器 SHA；测试明细同时绑定演练、目标计划、运行收据、双端来源与测试时间。已有结果不会覆盖；有失败、跳过、重试、缺场景、源码不匹配、明细不一致或数据验证尚未完成时不生成成功证明。最终 `restore verify` 必须同时收到三份相邻证据、原目标计划和 runner 根目录；维护 CLI 会从登记库构造权威上下文，由当前干净后端重新核验目标计划、运行中三进程、完整构建来源、runner 以及全部摘要，再登记成功。这一步仍会重新检查服务就绪和恢复时间，浏览器通过不等于最终恢复登记成功。最终核验使用统一 data 入口在编译时绑定当前后端 SHA；直接运行旧维护二进制会失败。若设置 `RYFRAME_PYTHON`，必须提供已验证解释器的绝对路径。核验器会以隔离模式重新执行 Python 环境检查，绑定解释器文件身份与摘要，限制输出和运行时间，并在前后再次核对解释器及双端 Git 来源。

### 参考环境的外部验收驱动

`cargo xtask check recovery` 调用私有的本机 MySQL、mysqldump、AWS CLI 和 Node 阶段程序准备演练数据与外部备份。先把源、目标的精确地址、不同 scope、数据库 ownership、运行收据目录和工具摘要写入忽略目录中的计划文件；字段校验以恢复计划规则为准。MySQL 使用计划内明确的客户端配置文件及其摘要，S3 凭据只引用环境变量。工具不创建或扫描数据库，源目标均需事先初始化。参考规模为 system 加十个普通租户、至少十万条实际岗位记录及至少 1 GiB 已登记上传对象，租户分布覆盖共享和独立目标；例如 256 个 4 MiB 对象。岗位记录属于控制库，租户业务表复制另由 Device 生成资源验收覆盖。

数据准备计划的 `dataset.request_interval_ms` 必须为 1000 至 5000 毫秒，限制每个固定客户端的实际 HTTP 请求启动频率；十一个租户可并发准备，每个租户使用自己的身份与地址。`dataset.timeout_seconds` 显式设置整阶段时限（1 至 604800 秒），例如参考规模预留 21600 秒；其他外部命令仍使用 1800 秒超时。收到 429 时保留失败，不通过重试或更换地址绕过限流。数据准备发生在备份与恢复开始之前，其耗时不计入恢复时间。

所有命令从对应干净后端工作树执行；`cargo xtask check recovery` 固定当前 `--backend-dir`，各阶段共用 `--plan <计划JSON>`。`plan` 默认只核对或输出计划，显式发布目标计划和其他写步骤必须传入 `--write`：

| 阶段 | 操作与输出 |
| --- | --- |
| `dataset` | 通过当前 API 建立套餐、十个租户、业务记录和关联对象，保存逐条创建证据与 `dataset/result.json`。 |
| `backup --inventory <清单JSON> --source-runtime <来源运行证明JSON> --source-quiescence <停止观察收据JSON> --source-export-result <已发布共享导出结果>` | 核对停止前实际干净构建、源侧业务复验、停止观察和已发布共享导出的同一库存及运行代次，精确导出数据和对象、核对摘要；`backup.json` 绑定共享导出身份、来源证明和 `backup/manifest.json` 文件摘要，随后用产品 `backup-register` 登记。清单使用共享导出的原始 inventory 并添加本次备份 `id`。 |
| `restore --target-plan <本侧目标计划> --runtime-registration <本侧运行登记> --backup-root <备份目录> --record <restore-begin记录>` | 先复核全部输入与产品计划精确一致性，持运行控制锁重新核对完整源码、B0 重建及目标四库五桶完整前像。每次写前核验三端口空闲，完成后采集完整后像。分别保存 `restore-base.json` 或 `restore-candidate.json`，绑定原计划、备份、运行记录、运行登记和前后像；复制完成后仍需产品数据验证和真实业务证明。 |
| `copy --backup-root <备份目录> --copy-id <独立副本ID>` | 创建保留原摘要的独立备份副本；先登记副本并执行 `restore-begin`。 |
| `damage --backup-root <副本目录> --artifact <清单内路径> [--missing]` | 仅损坏或删除指定副本产物，保留原备份；随后用 `restore-verify-data` 验证失败状态及告警。 |

发布本侧目标计划后，先用 `cargo xtask check recovery runtime register --plan <本侧参考计划> --target-plan <本侧目标计划> --output <新运行登记文件> --write` 登记从未启动的目标。产品 `backup-register`、`restore-begin`、后续验证的状态仍写入 `APP_CONFIG_DIR` 指定的源侧登记库，`--restore-config-dir` 只指定独立恢复目标；把 `restore-begin` 返回记录保存到新文件再传给外部恢复阶段。目标侧 ownership 与备份、恢复登记表必须完整保持初始化前像，不因登记操作获得例外。运行登记、完整资源像或任意产物漂移都会失败；失败记录保留后像复核结果，未知写入不能自动重放。

正式双侧恢复分别使用 `target_side: base` 与 `target_side: candidate` 的参考计划和独立工作目录。通过 `cargo xtask check recovery plan --plan <本侧参考计划> --backup-receipt <同一backup.json> --comparison-sources <双版本来源清单> --arm-input <本侧已发布arm结果> --fresh-target-verify <本侧观察目录/verify.json> --product-plan <产品RestorePlan文件>` 推导完整目标计划；加 `--output <新目标计划文件> --write` 才发布。它把 base 固定映射到 b0、candidate 固定映射到 b1，并绑定同一共享导出、原备份摘要、本侧初始化与完整 ownership、探针、前端 SHA 和产品恢复计划。目标计划分别记录维护初始化的 `maintenance_execution` 与比较产品的 `product_execution`；双侧可共享候选维护工具，B0 产品运行仍必须使用已登记适配源码和对应构建。产品计划的 `fault_at` 必须使用 UTC `Z` 格式，小数位按实际精度保留 0、3 或 6 位，保证经过产品序列化后仍逐字段一致。`plan --plan <本侧参考计划> --target-plan <已发布目标计划>` 重新核对全部绑定；未知字段、侧别混用、证据漂移或输出覆盖都会失败。

已有数据可以在原计划不变的前提下复验。先用 `cargo xtask check recovery check-existing --plan <原计划JSON> --side source` 核对源侧数据库 ownership 和已登记 API；随后运行 `cargo xtask check recovery dataset-prepare --plan <原计划JSON> --verify-existing <原dataset/result.json> --side source --write`，读取原岗位样本并下载校验全部登记对象，将标准输出保存到新的独立证据文件。`--write` 表示登录、注销会产生会话及审计副作用，业务记录和对象始终只读。省略 `--side` 时已有数据验证仍固定为 `target`；数据准备仍固定为 `source`，其他阶段不接受该选项。

验证结果记录实际侧、scope、原计划及数据收据摘要和只读动作范围，状态为 `existing_data_verified`，不表示恢复成功，也不单独证明源码干净。源侧复验结果不能用于目标恢复证明。

正式备份前，在同一配置下启动真实干净构建的 API 和 external Worker，使用 `cargo xtask check recovery source verify --plan <原计划JSON> --build-receipt <本次构建收据JSON> --dataset <原dataset/result.json> --output <新来源运行证明JSON> --write`。该命令会亲自执行源侧已有数据复验，在前后核对 API/Worker 的构建摘要、配置、创建身份、监听端口及就绪状态。

成功后停止全部生产者，再执行 `cargo xtask check recovery source quiesce --plan <原计划JSON> --source-runtime <来源运行证明JSON> --output <新停止观察收据JSON> --write`。该命令不终止进程；它核对同一代次已停止并记录实际观察时间。之后才能重新采集 Inventory，其 `quiesced_at` 使用该收据的 `observed_stopped_at`。给 `backup` 同时传入两份证明；备份前后均检查来源进程已停止、未换代，以及复验、实际停止观察、采集时间的先后关系。清单先采集、进程后来才停止的流程会失败。重新复验使用新的输出文件，失败日志保留；不改写原计划、数据收据和历史证据的摘要或完成时间。

B0/B1 正式对照前，使用 `cargo xtask check recovery source comparison-capture --help` 查看来源参数，再以 `comparison-capture ... --source-export-result <唯一导出外层结果> --output <新来源清单> --write` 生成严格的双版本来源清单。B0 必须分别提供原产品源码、已登记的工具适配源码和前端源码；B1 必须提供最终干净的双端源码。两侧都必须提供对应源码上的 v2 后端与前端生产构建收据，并共同绑定同一个已发布 `source-export` 的外层文件摘要及内部导出身份。`cargo xtask check recovery source comparison-verify --receipt <来源清单>` 只读重建并核对全部来源、构建产物、完整前端 `dist` 和导出证据；它不创建恢复 run、锁或业务写入，也不接受旧格式字段。

B0 后端构建仍由当前受信任工具执行，但 `--source-backend` 必须指向登记的适配提交，并同时提供 `--expected-head <适配 SHA> --adapter-contract legacy-stable-readiness-b0-v1 --product-backend <原 B0 绝对工作树>`。入口会从当前工具内嵌补丁重建适配树，核对原 B0、适配提交、补丁摘要和产品输入域，任一项不匹配都会在调用 Cargo 前失败；不能复制当前脚本到旧工作树或手工补写 v2 收据。

恢复浏览器套件逐一登录十一个租户读取备份前已有的岗位，下载并校验全部原有上传对象，再执行正常核心业务。故障与 Worker 重启场景继续由普通全栈套件执行。每阶段保留独立收据，失败不覆盖、不自动清理资源；只有最终产品 CLI 核算后的成功记录能证明 24 小时恢复点与 60 分钟恢复时间。

### 指标与告警

API 与 Worker 的备份采集器每 60 秒读取控制库汇总，整次采集最多等待 10 秒，输出到现有 Prometheus 指标接口。运行时监控页显示采集状态、必需及异常资源、最近恢复结果与耗时；未知、失败和陈旧状态不表示当前数据可用，保留的有效快照仅供诊断。备份故障独立展示和告警，不改变 API readiness。

`ryframe_backup_last_success_timestamp_seconds` 取所有必需目标的最旧有效备份采集时间，重新登记旧备份不会刷新恢复点。`ryframe_backup_resources` 使用固定的 required/missing/expired/invalid 维度，`ryframe_backup_collector_up` 表示采集成功与否，`ryframe_backup_collector_last_success_timestamp_seconds` 记录最近成功读取汇总的时间；后者缺失或超过 150 秒会触发采集告警，不能用它替代备份恢复点。恢复指标包括最新完成结果、耗时、恢复点年龄和 running/overdue 汇总。指标没有租户、端点或对象键标签。

`deploy/prometheus/ryframe-alerts.yml` 在 23 小时预警、24 小时告警，并分别检查缺失、过期、校验失败和采集失败。收到恢复失败或超时告警时先查看登记状态和保留的演练证据，不通过改登记时间消除告警。

将该规则文件只读挂载或复制到现有 Prometheus 的 `/etc/prometheus/rules/ryframe-alerts.yml`，在其配置中显式加载这个文件。下例中的两个 HTTPS 地址应替换为现有监控入口，分别转发到 API 的 `/api/v1/monitor/metrics` 和 Worker 健康端口的 `/metrics`；生产 Compose 中的 Worker 默认只在后端网络可见，需要由现有监控网络或反向代理提供访问。密钥文件只保存与 RyFrame `APP_MONITOR_METRICS_BEARER_TOKEN_FILE` 相同的 token 内容，路径以 Prometheus 所在主机或容器为准。

```yaml
global:
  scrape_interval: 30s
  evaluation_interval: 30s

rule_files:
  - /etc/prometheus/rules/ryframe-alerts.yml

scrape_configs:
  - job_name: ryframe-api
    scheme: https
    metrics_path: /api/v1/monitor/metrics
    authorization:
      type: Bearer
      credentials_file: /run/secrets/ryframe-metrics-token
    tls_config:
      ca_file: /run/secrets/monitor-ca.pem
    static_configs:
      - targets: [ryframe-api.internal.example:443]
  - job_name: ryframe-worker
    scheme: https
    metrics_path: /metrics
    authorization:
      type: Bearer
      credentials_file: /run/secrets/ryframe-metrics-token
    tls_config:
      ca_file: /run/secrets/monitor-ca.pem
    static_configs:
      - targets: [ryframe-worker.internal.example:443]
```

沿用现有 `alerting.alertmanagers` 配置，并将 `service=ryframe` 的 warning/critical 告警路由到运维通知渠道。先执行 `promtool check config /etc/prometheus/prometheus.yml` 与 `promtool check rules /etc/prometheus/rules/ryframe-alerts.yml`，通过后按现有平台的配置重载方式生效；Prometheus 支持 `SIGHUP`，也支持在启用生命周期接口后通过 `POST /-/reload` 重载。[配置与重载方式](https://prometheus.io/docs/prometheus/latest/configuration/configuration/)

重载后在 Targets 中确认两个抓取目标均为 UP，在 Rules 中确认 `ryframe.application` 规则组加载且健康，并查询 `ryframe_backup_collector_up`、`ryframe_backup_resources` 和 `ryframe_backup_last_success_timestamp_seconds` 检查实际采集结果。API 与 Worker 各暴露一份部署汇总，按 Prometheus 的 `job`、`instance` 区分采集进程，不能将两份计数相加。最后通过隔离演练产生真实失败状态，核对告警进入 Alertmanager 并送达通知渠道；仅通过配置检查不作为通知送达证据。

## 发布门禁排障

协调版本 tag 同时触发双方日常 CI 和 Extended CI。后端的 core 与 Device 全栈任务均检出配套前端 tag，产物内记录双方精确 SHA、run ID 和 attempt；Device 产物另附从干净源码生成隔离工作树的收据。Release 在校验阶段和实际创建 Release 前分别检查四组最新运行、两套全栈产物及必需 job；缺失、失败、取消、超时、错误 SHA、跳过必需 job 或最新重跑未成功都会阻断发布。

检查 Release 保存的 `release-ci-evidence.json` 与 `release-final-evidence.json`，按其中 run ID 和 attempt 定位失败。默认总等待上限为 5400 秒，GitHub API 分页和单次调用共享截止时间。修复后对同一目标源码重跑相应 CI，再重新运行 Release；不移动已有 tag，也不以其他提交的成功结果替代。发布资产仍为 GitHub 源码归档。

## TLS 证书

检查到期时间、主机名、完整链和私钥权限。轮换后验证 MySQL、Redis、对象存储、OTLP 与 Nginx；不要关闭证书校验作为长期修复。

## 故障记录

隔离参考夹具服务通过 `cargo xtask check recovery fixture services status --review <审阅收据> --environment <bootstrap.json>` 查看只读状态，输出原账本、控制器身份及下一合法操作。关闭时改用 `close` 并显式追加 `--write`，按 Redis、RustFS 顺序回收原进程树，确认端口释放；数据库、对象及服务目录保留。若宿主在正常关闭收据发布前回收了整棵服务树，`status` 仅在全部登记身份消失、端口关闭且 ownership 未变化时报告外部终止，并返回当前 `state.json` 的 owner binding；先用该文件执行 `recover --owner-binding <state.json> --write` 追加独立核对证据，再对新的 state binding 执行 `restart --owner-binding <state.json> --write`。重启在新代次目录保存请求、进程树和前代 lineage，不补造旧代次的正常关闭证明。新首代 RustFS 在产品代码执行前建立 Job Object 或 Unix session；独立的私有成员监督器持有完整成员的内核句柄，确认全部退出后才签发完成证明。历史收据缺少进程树归属时不能补造完整关闭证据。

死亡控制器通过同一入口的 `recover --review <审阅收据> --environment <bootstrap.json> --owner-binding <状态输出中的 owner 文件绑定 JSON> --write` 显式恢复。它只清理已核对的本地控制锁并追加账本，不停止服务或重放未收尾操作；PID 已复用、来源改变、未知启动或关闭结果均须保留现场核对完整前后像。已关闭代次不能再签发 fresh-target 请求；已登记目标只接受严格核验过的已收尾生命周期追加。

记录时间线、影响租户、请求或任务 ID、根因、处置和验证结果。配置字段以配置结构为准，指标和告警以部署资产为准，不在本文维护重复清单。
