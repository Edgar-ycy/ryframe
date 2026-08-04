# RyFrame 0.5.0–0.7.0 分阶段整改追踪

本文以《RyFrame 0.5.0–0.7.0 分阶段整改计划》为唯一范围。状态含义：

- `已完成`：实现与对应自动化验证均已具备。
- `部分完成`：已有基础实现，但尚缺计划要求的能力或验收。
- `未开始`：尚未发现实现或验收证据。
- `待核验`：实现存在，但尚未完成计划要求的运行环境验证。

## 0.5.0：工程闭环

| 条目 | 状态 | 当前证据与剩余工作 |
| --- | --- | --- |
| 工作流、编码与仓库卫生 | 部分完成 | 后端当前已通过全工作区全目标 `check`、Clippy、格式、111 项 Python 策略测试、架构检查、feature matrix、`cargo audit` 与 `cargo deny`；28 个中文测试函数已统一改为英文 `snake_case`，21 个工作区成员通过编译器 `non_ascii_idents = "forbid"` 禁止非 ASCII 代码标识符。Qodana 报告中的 2 项重复定义、2 项无效依赖、108 项冗余限定名和 2 项 trait 实现顺序问题已修复；后续报告只剩 7 项已登记的依赖升级提示，按风险进入 `0.7.0` 升级评估，不在 `0.6.0` 混入。前端源码、workflow、依赖、架构、契约、ESLint、Stylelint、typecheck、67 个文件的 329 项覆盖率测试、生产构建、包体预算、14 项开发态 E2E 和 1 项 `dist` 生产烟测均已通过。CI 已删除重复的 Windows、coverage、Nightly 与 Rust 测试作业，第三方 Action 固定到提交 SHA 或容器摘要。根目录误建的 `.pnpm-store` 已不存在；仍待提交最终前端同步结果并取得托管 CI 证据。 |
| 稳定版联合发布 | 部分完成 | 前后端 `v0.5.1` annotated tag 已推送，且本地联合发布输入校验通过。稳定发布已收敛为双仓不可变身份校验和纯源码 GitHub Release，不再构建平台镜像、签名或自定义附件；仍需取得精简后托管发布的完成证据。 |
| 固定来源 OpenAPI 契约 | 部分完成 | 固定 40 位提交、离线校验和固定上游校验机制已具备。前端最终快照、来源元数据和生成类型已同步，删除旧 `/all` 路径、加入受限 `/options?q&limit`，并将全部路径与查询 ID 锁定为字符串传输；本地来源校验、契约检查、生成幂等性和完整门禁均已通过。仍须提交最终同步结果，并在后端提交推送后取得固定上游校验证据。 |
| 开发工具链与 `xtask` | 已完成 | 工具链、`xtask` 和统一配置入口已具备；已在 `v0.5.0` 双仓干净克隆中运行完整联合 `cargo xtask check --scope all`，工作区无差异。 |
| 0.5 发布验收 | 部分完成 | 本地后端全工作区门禁、前端完整门禁、固定上游 OpenAPI 校验与干净双仓标签克隆验收均已通过；`v0.5.1` 双仓稳定标签已推送，仍需取得精简后托管 CI 与纯源码联合发布的完成证据。 |

## 0.6.0：正确性、安全与可维护性

| 条目 | 状态 | 当前证据与剩余工作 |
| --- | --- | --- |
| 统一 API 响应与分页契约 | 已完成 | 响应统一为 `message/data/request_id/error_key/details`，分页数据进入 `data`；C1 已删除 11 个无上限列表及其前后端调用，不保留旧接口兼容。角色和用户候选项使用受限 `/options?q&limit`，Service/Repository 原始分页数值回流已有架构守卫；13 个分页 operationId、两个 `/options`、参数边界、11 个旧路径和字符串 ID 均有精确契约守卫。响应中间件只对 JSON 做 16 MiB 有界缓冲，显式旁路 WebSocket、下载和文档流，非信封 JSON 失败关闭并保留原错误响应头；`/version` 已使用强类型信封。后端 workspace、`Cargo.lock` 与 OpenAPI `info.version` 已统一为 `0.6.0`，前端最终固定来源同步和完整本地门禁均已通过。 |
| 安全、Markdown 与幂等 | 已完成 | 公告 API 已删除旧 `content` 字段并统一为 `content_markdown`，按 1–60,000 个 UTF-8 字节校验；限制由 OpenAPI 扩展发布，前端预览继续禁用原始 HTML 并经 DOMPurify 清洗。配置错误与未知内部错误只返回通用 `500` 消息，废弃的 `X-XSS-Protection` 已删除。幂等存储键已收敛为租户、用户和原始客户端键，完整指纹绑定方法、真实规范化路径、排序查询和 body SHA-256，同键不同指纹返回 `409`。实现、架构守卫以及隔离 MySQL/Redis 并发与故障测试均已通过；在线用户 Redis 旧触碰并发守卫也已在最终通用 Docker 运行时验收中通过。 |
| 增量迁移与独立 Worker | 部分完成 | 迁移、任务表、独立 Worker 与生产外置限制已实现；`sys_outbox_event` 具备事务写入、租约领取、退避重试、死信及与后台任务原子投递，`ryframe-worker --once` 可用于单轮演练。过期租约回收已与并发领取事务解耦，Worker 会在执行时复核任务创建者当前权限与数据范围，导出资源使用稳定主键游标批处理。隔离 Docker 已通过 19/19 迁移、结构校验、API 与独立 Worker 就绪、真实异步导出，以及 Worker A 中断、30 秒租约自然过期、生产恢复和 Worker B 接管；仍须在部署环境完成 API/Worker 正式切换演练。 |
| 后台导出与通用任务接口 | 已完成 | 用户、角色、岗位、参数配置、字典类型、操作日志和登录日志已统一为 `POST /system/{resource}/exports`，创建请求必须携带 `Idempotency-Key` 并返回 `202`；任务统一从 `/common/jobs` 查询、取消和受控下载，结果 24 小时过期清理。前端创建、轮询和下载已迁移到统一 mutation，具备同一 `AbortSignal`、集中错误处理、重复提交守卫和按租户精确失效；相关导出专项测试及管理组合式函数回归测试已通过。单一 opt-in 精确验收已在隔离 MySQL 与 RustFS 上真实通过，覆盖恰好 10 万用户、游标导出与 XLSX 回读、Worker 租约接管、PUT 503 生产重试、DELETE 503 跨轮恢复和无残留清理；最终生成类型也已随 `0.6.0` 后端 OpenAPI 同步并通过前端门禁。 |
| 权限快照、缓存与文件完整性 | 已完成 | 权限快照迁移与授权边界、CACHE-C 数据库 `BIGINT` 权威版本、同事务配置写/版本递增/Outbox、Redis 同槽固定键与精确十进制 Lua 比较均已实现并通过静态、本地单元与隔离运行验证。FILE-A 已在隔离 MySQL 与 RustFS 上完整通过：旧 schema 和真实 MD5 碰撞对象种子、两次迁移闭锁、SHA-256 回填 dry-run/apply、旧预留排空 dry-run/apply、最终迁移、旧列/索引删除及对象隔离断言全部成功；通用验收中的 Redis refresh CAS、Redis 故障恢复、在线用户旧触碰并发守卫、真实 RustFS S3 读写删、对象存储故障恢复与过期清理链路也已通过。 |
| 前端数据层、错误处理与可访问性 | 已完成 | TanStack Query、会话恢复、安全渲染、真实 CRUD、动态权限路由及通用异步导出迁移均已落地；API origin 与 OpenAPI 生成前缀已分离，前缀扩展由同步和契约检查严格验证。`package.json` 与 OpenAPI 已统一为 `0.6.0`，契约快照和生成类型移除旧 `/all` 接口并加入角色、用户受限 `/options`。源码、workflow、依赖、架构、契约、ESLint、Stylelint、typecheck、67 个文件 329 项覆盖率测试、生产构建、包体预算、14 项开发态 Playwright E2E 和独立 `dist` 同源生产烟测均已通过；E2E Mock 已删除旧 `msg` 兼容转换并接受生成契约类型约束。 |
| 0.6 发布验收 | 部分完成 | 后端版本面和 `0.6.0` OpenAPI 已生成，本地后端静态门禁、专项测试、架构检查、feature matrix、FILE-A 与通用 Docker 真实闭环验收均已通过；最终通用验收包含 6 个严格串行测试阶段、数据库重置与 19/19 迁移校验、API/独立 Worker 启动、22/22 跨进程烟测及 5/5 Docker 资源清理，终态 `run.json` 为 `passed` 且无错误。前端已同步最终 `0.6.0` 契约，并通过源码、workflow、依赖、架构、契约、Lint、typecheck、覆盖率、生产构建、包体预算、开发态 E2E 和 `dist` 生产烟测。仍待提交最终同步结果、双仓精确提交 CI、同名 annotated tag、纯源码联合发布及部署环境 API/Web 正式切换。 |

当前结论：`0.6.0` 的本地代码实现、FILE-A、通用 Docker 真实依赖运行验收、前端固定来源 OpenAPI 同步和完整门禁均已闭环；当前尚缺最终同步结果提交、双仓精确提交托管 CI、固定上游校验、同名稳定标签、纯源码联合发布及部署环境正式切换。在这些证据齐备前不得声明 `0.6.0` 发布完成。

## 0.7.0：架构与正式扩展能力

| 条目 | 状态 | 当前证据与剩余工作 |
| --- | --- | --- |
| Crate 边界与最小依赖 | 已完成 | `kernel`、`i18n`、`excel`、`mail` 等拆分已存在，旧公共兼容包已删除。API 拥有全部公开 DTO 并对 Service 模型做穷尽所有权转换；Service、Generator、Utils 运行依赖已移除 Utoipa。`config/feature-matrix.json` 是最小/最大 feature 组合唯一注册表，CI 校验元数据，本地 `cargo xtask feature-matrix` 执行完整编译。 |
| 依赖升级与生态收敛 | 部分完成 | Qodana 在后端提交 `45db06e69d1f7593aeb06dc8699d9d5ce4426ab6` 上只报告 7 项新版本提示，未发现新的代码质量问题。`rust_xlsxwriter` 作为低风险首波，随后独立处理 `validator`、`tower-http`、`jsonwebtoken` 与 `syn`，每波均要求锁文件、专项回归和完整门禁；`base64 0.23` 默认 SIMD `unsafe` 且会与依赖树中的 `0.22` 并存，`password-hash 0.6` 又缺少兼容的稳定版 `argon2` 依赖族，二者不得为了消除版本提示强行升级，须等稳定生态收敛后再处理。仓库不全局禁用 `NewCrateVersionAvailable`，确保后续新提示仍可见。 |
| i18n、OTel、URL 版本与读副本 | 部分完成 | 双语、语言协商、`Content-Language`、`Vary: Accept-Language`、OTel、`/api/v1` 与读一致性选择已实现；未知版本和无版本 API 路径已统一为 JSON `404`。API 与独立 Worker 已共用日志初始化，文件模式按日滚动并执行有界保留，生产容器默认 stdout。两类进程的 `/readyz` 均只读后台探测生成的有时效内存快照，过期时 fail-closed，请求路径不再访问依赖；Worker 对象存储明确为 `not_required`。隔离环境已验证健康副本参与轮询读取，副本不可达时 API 仍就绪并切换为 `primary_fallback`。OTel 已通过不响应的本机回环端点完成确定性故障验收：业务与 readiness 不受影响、失败使用无动态标签指标和固定阶段日志、退出 flush 最多等待 5 秒。仍需在托管环境完成真实 Collector 故障演练和完整端到端验收。 |
| 通用消息中心 | 待核验 | 消息表、票据、服务端持久化收件箱、ack/read 与前端连接均已具备；容量与生命周期由强类型 `MessagingConfig` 注入，收件人通过同事务 `INSERT … SELECT` 固化。现有记录描述过一次双实例链路，但仓库内尚缺可重复执行的证据；仍需验证断线补拉、去重、ticket 过期与重放、错误 Origin、Redis 故障、1013 慢消费者、90 天清理和双实例租户隔离。 |
| 源码发布与生产 Compose | 部分完成 | GitHub Release 已明确只保留平台自动生成的源码 ZIP/TAR，不分发任何平台镜像或自定义附件；生产 Compose 已将 API、迁移和独立 Worker 与外部托管依赖分离。仍需由部署环境完成稳定标签源码构建、扫描、摘要固化和升级演练。 |
| 0.7 发布验收 | 部分完成 | 双实例消息、读副本故障回退与 OTel 本地确定性故障验收已完成；仍需完成托管 OTel Collector 故障以及生产源码构建、升级和回滚演练。 |

## 最近本地验收记录

2026-08-04 先完成导出运行验收的代码接入（当时尚无真实运行证据）：

- `export_runtime_acceptance_test` 以函数级 `--exact --ignored --test-threads=1` 唯一接入 `scripts/runtime_acceptance.ps1`，执行顺序固定在真实 RustFS CRUD 之后、数据库 reset 之前；普通工作区测试不会重复运行该 ignored 测试。
- RustFS endpoint、凭据与区域由脚本通过可恢复的测试变量固定，`NO_PROXY` 固定为本机回环地址；测试端只接受精确的 `http://127.0.0.1:<port>`，代理客户端也显式绕过系统 HTTP 代理。
- 2026-08-04 首次重跑在启动 Docker 前暴露 Windows PowerShell 5.1 不接受空备份路径的 `File.Replace` 行为；`run.json` 写入现改用同目录唯一 `.tmp/.bak`、四参数原子替换和 `FileStream.Flush(true)`，并区分提交失败与提交后的临时文件清理警告，避免磁盘终态与进程退出状态矛盾。生产函数连续两次写入的真实 Windows PowerShell 5.1 回归已通过，验证替换后的 JSON、UTF-8 无 BOM、末尾 LF 及无临时残留。该次运行没有启动容器或执行测试 SQL，仍须重新完整运行。
- 2026-08-04 第二次重跑确认 `desktop-linux` context 配置正确，但 Docker Desktop 未启动且 Linux Engine 命名管道不存在；脚本现会在取得 Compose 所有权前显式探测本机 daemon 并记录服务端版本。daemon 初始不可用时直接给出启动提示，不再执行必然失败的 `compose down`；Compose 已开始创建资源后的失败仍保留完整清理。
- 同日启动 Docker Desktop 时，其自身一度被 `C:\Users\edgar\AppData\Local\Docker\run\dockerInference` 的旧 AF_UNIX reparse point 阻塞；该环境阻塞随后已解除，后续完整验收使用同一 `desktop-linux` context 成功运行。
- Python 策略与源码卫生门禁已锁定精确测试 target、函数名、调用次数、阶段顺序、RustFS 回环隔离和 ignored allowlist；仍须运行完整隔离验收并保留 `run.json` 与 transcript，才能把 10 万条规模、租约接管、对象存储故障恢复和过期清理标记为已验证。

> 以上为最终运行前的阶段性记录；其中尚缺的规模、租约、故障恢复和清理证据已由下述最终通用运行时验收全部补齐。

2026-08-04 通用运行时验收最终完整通过：

- 证据目录为 `target/runtime-acceptance/20260804015117-36300-e04ef5dfdd40`；Windows PowerShell 5.1、Docker Desktop Linux Engine `29.6.2`，开始于 `01:51:17`，完成于 `01:58:25`。
- 脚本按固定顺序完成工作区测试、Redis refresh CAS、在线用户旧触碰并发守卫、API 隔离集成、真实 RustFS CRUD，以及 `export_runtime_acceptance_covers_scale_takeover_storage_recovery_and_cleanup` 精确 ignored 测试；最后一项覆盖 10 万条 XLSX 导出回读、Worker 租约接管、PUT/DELETE 503 恢复与过期对象清理。所有阶段退出成功后才进入数据库重置和跨进程烟测。
- API 与独立 Worker 跨进程烟测 22/22 通过；同一 Compose project 的 3 个容器、1 个数据卷和 1 个网络清理 5/5。`run.json` 终态为 `passed`，`error=null`、`cleanup_errors=[]`，证据目录无 `.tmp/.bak` 残留。

2026-08-04 完成 `0.6.0` 前端固定来源 OpenAPI 同步与本地门禁：

- 前端首轮以后端提交 `9d6f1cedafa7d34c340825f6a2a1f04f210b71c7` 为唯一来源同步契约；当时 `openapi/source.json` 记录的 SHA-256 为 `e8ca061ca1472566cab5047877b4fcb565cafb7b1897b3328e1a71a50db74064`。
- 前端 OpenAPI `info.version` 已更新为 `0.6.0`，快照及生成类型已删除旧 `/all` 路径，并加入角色和用户的受限 `/options?q&limit` 契约，不保留旧接口兼容写法。契约守卫已从脆弱的查询数量阈值改为 21 个必要受限查询 operationId 和 11 个旧无上限路径的精确断言。
- 本地来源校验、源码卫生、workflow、依赖、架构、契约、ESLint、Stylelint、typecheck、67 个文件的 329 项覆盖率测试、生产构建、包体预算与 14 项 Playwright E2E 全部通过。E2E 使用固定同源 API origin，补齐消息收件箱和未读数 Mock，并消除代码生成预览将 `v-loading` 挂到 `ElDialog` 非元素根节点产生的运行时告警。
- 最终假绿审计发现首轮快照仍将 35 个 Snowflake 路径 ID 发布为 `integer/int64`；后端已将其统一为字符串传输，并用 42 个路径 ID 操作回归测试及前端参数扫描守卫锁定。上述首轮哈希不作为最终发布来源，最终提交身份只记录在前端 `openapi/source.json` 和发布清单中。
- 同步结果尚未提交；后端提交推送前也无法取得 `api:check:upstream` 的固定上游验证，因此这些远程证据仍保留为发布前置条件。

2026-08-04 Qodana 与代码标识符静态收口：

- Qodana 基于后端提交 `61ecc984cbc426ba53059b42311808513e8a3924` 报告 121 项新增问题；其中 114 项可直接修复的问题已完成，包括 2 项重复定义、2 项无效依赖、108 项冗余限定名和 2 项 trait 实现顺序问题。
- 28 个中文 Rust 测试函数已改为英文 `snake_case`；根工作区使用 `non_ascii_idents = "forbid"`，21 个成员 crate 全部继承该 lint，防止非 ASCII 代码标识符再次进入。
- `jsonwebtoken`、`base64`、`password-hash`、`validator`、`rust_xlsxwriter`、`tower-http` 与 `syn` 的升级提示涉及主版本、安全边界或未稳定 API，不与 `0.6.0` 收口混合，转入 `0.7.0` 独立升级、迁移与回归波次。
- 最终复核通过全工作区全目标 `cargo check`、Clippy、格式、全工作区 `lib/bins` 测试、111 项 Python 策略测试、源码卫生、依赖策略、权限路由和架构检查；仅 1 项要求 Docker Redis 的测试按设计忽略。
- 后续 Qodana 报告以提交 `45db06e69d1f7593aeb06dc8699d9d5ce4426ab6` 为 revision，仅剩上述 7 项 `NewCrateVersionAvailable`，没有新增实现缺陷。升级评估确认 `rust_xlsxwriter` 可优先处理，`validator`、`tower-http`、`jsonwebtoken` 与 `syn` 必须独立回归；`base64` 与 `password-hash` 分别受重复依赖、默认 SIMD `unsafe` 和稳定版 `argon2` 尚未衔接限制，当前不做冒进升级，也不通过全局关闭检查掩盖。

2026-08-03 完成本轮静态收口：

- `cargo check --locked --workspace --all-targets`、全工作区 Clippy、格式检查与两仓 `git diff --check` 通过。
- `cargo xtask feature-matrix` 已实际编译 `ryframe` 与 `ryframe-http` 的最小/最大 feature 组合。
- 全工作区 `cargo test --locked --workspace --lib --bins` 通过；其中 API 单元测试 58 项，Service 单元测试 63 项通过、1 项 Docker Redis 测试按设计忽略；当次 Python 策略测试 110 项和架构检查通过，后续 Qodana 路由别名守卫加入后增至 111 项。
- 后端 workspace 与 `Cargo.lock` 已统一为 `0.6.0`，后端 OpenAPI `info.version` 已生成 `0.6.0` 并通过自身快照测试；前端 `package.json` 已为 `0.6.0`，但契约快照和来源元数据必须等待后端真实提交 SHA 后再同步，不以工作区 HEAD 冒充已提交契约来源。
- 前端普通全量单元测试 67 个文件、329 项全部通过；核心业务覆盖率为语句 83.97%、分支 74.03%、函数 76.79%、行 87.71%，并由动态清单守卫防止关键模块静默逃逸。`pnpm lint`、`pnpm lint:styles`、`pnpm check:workflows`、`pnpm check:dependencies`、`pnpm check:architecture` 与 `pnpm check:sources` 通过；完整 typecheck、生产构建和 E2E 必须在最终 OpenAPI 同步后重跑。
- 根目录误建的 `.pnpm-store` 已不存在；最终提交前仍须随完整门禁再次确认两仓源码卫生。
- 两套隔离运行时验收入口 `scripts/runtime_acceptance.ps1` 与 `scripts/file_a_acceptance.ps1` 已实现，并通过 PowerShell AST、独立策略守卫及源码卫生静态验证；前者覆盖 MySQL、Redis、RustFS、迁移、API/Worker 与烟测，后者覆盖 FILE-A 旧 schema、维护 CLI 和终结迁移闭环。
- 上述两套脚本均已在本轮启动隔离 Docker 并执行测试 SQL 与真实 RustFS 对象操作；FILE-A 已完整通过，通用运行时验收仍须在修复下述测试目标后重新从头运行。
- 2026-08-03 首次运行通用验收时，三项依赖均健康，工作区测试已执行到 API 集成测试并得到 30 项通过、2 项失败、2 项按设计忽略；已修复普通资料更新错误推进授权版本及公告测试仍发送旧 `content` 字段的问题。由于脚本按失败即停，后续 Redis、RustFS、迁移及 API/Worker 烟测仍须重新完整运行。
- 同日第二次运行 FILE-A 验收已完整通过：旧 schema 导入、两个真实 MD5 碰撞对象、两次闭锁、SHA-256 回填、旧预留排空、最终迁移与 schema/对象隔离断言全部成功，证据保存在 `target/file-a-acceptance/002333ce4d15`，专属容器和网络已清理。
- 通用验收第二次运行已借助 `--no-fail-fast` 执行完全部工作区测试目标，仅剩重复创建 `sys_outbox_event`、分页 doctest 返回类型及 AutoFill doctest 未初始化 Snowflake 三项失败；三项均已按当前契约修复，全工作区 doctest 已在本地复跑通过。通用脚本后续 Redis、RustFS、迁移及 API/Worker 烟测仍须再次完整运行。
- 通用验收第三次运行已通过全部工作区测试与 doctest、真实 MySQL 仓储/服务/迁移测试、Redis refresh CAS、Redis 故障恢复、RustFS S3 读写删、数据库重置、19/19 迁移账本和最终结构校验；API 与独立 Worker 跨进程烟测为 21/22，通过后的 Docker 容器、网络和数据卷清理为 5/5。唯一失败项的上传实际返回 HTTP 200，原因为烟测仍读取已删除的 `file_info`；现已改为精确校验 `file_id/file_name/file_path/file_url` 平铺结构并直接使用同源 `file_url` 下载，同时将遗漏的在线用户 Redis `stale_touch` 测试纳入第 5 个运行时专项阶段。取得最终全绿日志前仍不将通用运行时验收标记完成。
- 通用验收的 `run.json` 已补齐 `starting/running/failed/cleanup_failed/passed` 终态，使用同目录临时文件原子替换；主流程失败优先保留原始错误，成功提示只会在进程、Compose、环境和日志收尾且终态 `run.json` 提交成功后输出，提交后的辅助文件清理失败仅告警。该新版终态证据仍须随下一次真实运行验证，不能用旧格式的三次记录反向证明。

> 上述条目记录的是 2026-08-03 的中间失败和修复过程，其中所述“仍须重新运行”均已由 2026-08-04 最终完整通过的通用运行时验收满足。

2026-07-29 已在独立 Docker 测试服务上完成以下验证：

- 后端执行 `cargo xtask verify --scope backend`，通过源码卫生、依赖与权限路由策略、架构检查、全工作区测试、集成测试和 doctest。
- 前端执行 `pnpm check`，通过 OpenAPI 契约、静态检查、类型检查、231 项单元测试、生产构建、包体预算和 14 项 Playwright 端到端测试。
- 前后端工作流已通过 Actionlint；第三方 Action 的固定提交与容器摘要由后端架构守卫和前端工作流守卫验证。
- `v0.5.0` 后端与前端已分别从干净克隆完成后端全工作区构建、前端锁定依赖安装、OpenAPI 契约、类型与生产构建，以及联合 `cargo xtask check --scope all`；两仓均未产生版本控制差异。
- 本机若有其他项目占用默认测试端口，应使用 `RYFRAME_TEST_MYSQL_PORT`、`RYFRAME_TEST_REDIS_PORT` 和 Compose 端口覆盖启动独立的 RyFrame 测试服务；完整 `RYFRAME_TEST_MYSQL_ADMIN_URL` 仍可用于远程或自定义 MySQL，禁止复用或改动其他项目容器。

以下验收必须在已授权的托管或生产环境执行，不能由本地代码提交替代：托管 CI 完整运行证据、稳定标签纯源码联合发布、托管 OTel 导出器故障演练，以及生产源码构建、迁移切换和回滚演练。

## 执行顺序

1. 收口 0.5 双仓契约与干净克隆发布演练。
2. 完成 0.6 统一响应契约，再实施 Outbox、导出任务和 Worker 验收。
3. 完成 0.6 前端与生产切换验收。
4. 收口 0.7 架构、i18n、OTel、读副本与消息中心的运行验收。
5. 验证 0.7 纯源码 Release 和生产环境独立构建交付，完成稳定版联合发布演练。
