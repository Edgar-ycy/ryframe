# 生产监控与值班手册

> 最后核对：2026-08-13

本文档定义 RyFrame 生产环境的最低监控、告警和处置要求。告警规则模板位于
`deploy/prometheus/ryframe-alerts.yml`。模板中的阈值是初始值，上线后应依据容量测试
和至少两周的生产基线调整，但不得在没有替代保护的情况下直接删除告警。

## 1. 生产暴露边界

- `APP_API_DOCS_ENABLED=false` 是生产强制配置；Nginx 还会对 Swagger UI 和 OpenAPI
  JSON 返回 `404`。需要排查契约时使用仓库中的 `openapi/openapi.json`，不要临时对公网
  开启运行时文档。
- `APP_MONITOR_METRICS_BEARER_TOKEN_FILE` 必须指向密钥管理系统挂载的 UTF-8 secret 文件，内容至少 32 字节，独立于
  用户 JWT 密钥并定期轮换。Nginx 仅允许 Prometheus/VPN 网段访问 metrics 路径，应用
  再校验 Bearer Token。
- `/livez` 可用于进程存活探测；后台任务会检查关键依赖，`/readyz` 只读取内存快照，
  不会在请求路径执行 SQL、Redis 或对象存储网络调用。公网负载均衡器仅需访问
  探针，不应访问 metrics、监控详情接口或 API 文档。
- 当前只支持 Redis 7 standalone，不支持 Redis Cluster。多 API 实例必须连接同一个
  `required` Redis；内存会话后端仅保证单进程一致性，不得作为多实例生产降级方案。

Prometheus 推荐使用文件型 secret，避免 Token 出现在命令行和配置仓库：

```yaml
scrape_configs:
  - job_name: ryframe
    scheme: https
    metrics_path: /api/v1/monitor/metrics
    authorization:
      type: Bearer
      credentials_file: /run/secrets/ryframe_metrics_token
    static_configs:
      - targets: [api.example.com]

  # `jobs.mode=external` 时，队列深度、死信和任务耗时只由独立 Worker 进程采集。
  # 此端点只能经内网直连，不能通过公网 Nginx 暴露。
  - job_name: ryframe-worker
    scheme: http
    metrics_path: /metrics
    authorization:
      type: Bearer
      credentials_file: /run/secrets/ryframe_metrics_token
    static_configs:
      - targets: [ryframe-worker.internal:9091]
```

轮换 Token 时按实例分批：Prometheus 临时建立两个使用不同 secret、且分别直连新旧
实例池的 scrape job；更新一批实例后确认新 Token 抓取成功，再更新剩余实例并删除旧
job。不能让两个 Token 经同一个随机负载均衡目标抓取，否则会产生间歇性 `401`。任何
时候都不能把 Token 写入 URL。

## 2. 采集依赖

| 能力 | 数据源 |
| --- | --- |
| HTTP 错误率、P95/P99、Redis 降级、refresh 重放、限流 | RyFrame `/api/v1/monitor/metrics` |
| 后台任务队列深度、最老等待时间、死信、任务耗时与消息保留清理 | 外置 `ryframe-worker` 的内网 `/metrics`（默认 `9091`） |
| MySQL 连接容量 | `prometheus/mysqld_exporter` |
| 主机与存储卷磁盘 | `prometheus/node_exporter` |
| TLS 到期 | `prometheus/blackbox_exporter` HTTPS probe |
| 备份成功时间 | 备份任务通过 node exporter textfile collector 或 Pushgateway 发布 |

备份指标必须只在备份摘要校验和隔离环境恢复演练成功后更新；仓库不提供 `deploy.sh`：

```text
# HELP ryframe_backup_last_success_timestamp_seconds Last validated backup Unix timestamp.
# TYPE ryframe_backup_last_success_timestamp_seconds gauge
ryframe_backup_last_success_timestamp_seconds 1784937600
```

如果未部署上述 exporter，对应告警不是“已覆盖”。上线清单必须记录每条规则的
Prometheus 查询结果和 Alertmanager 测试通知。

生产使用 `jobs.mode=external` 时，API 实例不会采集后台任务队列指标；必须为每个
`ryframe-worker` 建立上述独立 scrape target，并在网络策略中仅允许 Prometheus/VPN
访问 `jobs.health_port`。`/livez` 和 `/readyz` 同样通过该内网端口探测，禁止将其经
公网 Nginx 转发。独立 Worker 的后台快照只要求 MySQL 与 required Redis，对象存储
标记为 `not_required`；对象存储仍在 Worker 启动时完成导出 bucket 校验。

规则模板中的 `runbook` annotation 使用仓库内相对路径。部署时如果已将本文档发布到
内部文档站，可将其转换为 Alertmanager 支持点击的绝对 `runbook_url`。

## 3. 通用值班流程

1. 确认告警仍在触发，记录开始时间、版本 SHA、实例、租户影响和最近发布/配置变更。
2. 先止损：摘除异常实例、暂停发布、收紧入口或切换已验证的回滚版本；不得为了恢复
   指标而清库、清 Redis 或删除对象。
3. 从请求 ID 串联 Nginx、应用、MySQL、Redis 和对象存储日志。日志及截图不得包含
   access token、refresh token、metrics token、密码或对象存储密钥。
4. 恢复后观察至少 30 分钟，确认错误率、延迟、连接数和依赖状态回到基线。
5. P1/P2 事故在 2 个工作日内完成复盘，记录时间线、根因、影响、处置和防复发任务。

建议分级：安全事件、不可登录、跨租户风险、数据不可恢复或整体 5xx 为 P1；关键功能
明显退化为 P2；容量趋势或局部限流为 P3。

## 4. 告警处置

### HTTP 5xx 与高延迟

先按 `path`、`status`、实例和版本拆分。检查 `/readyz`、MySQL 连接、Redis 降级、
对象存储延迟和进程 CPU/内存。仅单实例异常时先从负载均衡摘除；与新版本强相关时按
发布指南回滚。不要用无限扩容掩盖下游连接池耗尽。

### 数据库连接容量

比较 MySQL `Threads_connected`、`max_connections`、慢查询、锁等待与应用实例数。
先暂停批量导入/生成等非关键高并发任务，确认连接泄漏或慢 SQL，再决定扩池。扩容前
必须核算 `应用实例数 × 每实例最大连接数 + 运维/迁移预留`，不得超过数据库安全上限。

### Redis 降级

检查网络、ACL/TLS、内存、`maxmemory-policy=noeviction`、AOF 状态和延迟。生产 Redis
为 required；不得通过切换 optional 来消除告警。恢复后确认 refresh 会话、撤销状态、
幂等记录和分布式锁仍有效，并执行登录/刷新/登出冒烟。

确认 Redis 拓扑仍为 standalone；不要在事故中把配置临时切向 Cluster 节点或 Cluster 代理。
会话接口依赖 Redis 乐观事务原子维护 Refresh Family、租户索引和租户用户索引，Redis 不可用时
`/api/v1/auth/sessions`、单设备撤销和批量撤销应明确返回 `503`。这时不能宣告撤销完成，也不能
通过手工删除展示 metadata 代替权威 Family 撤销。

### Refresh Token 重放

按安全事件处理。保全相关请求 ID、账号、租户、来源 IP 和 user-agent；确认 token family
已被吊销，必要时吊销该用户全部会话并通知安全负责人。禁止在工单或聊天中复制原始
Token。排查日志泄漏、代理查询参数、浏览器插件和客户端并发刷新。

### 登录设备与会话撤销

设备列表和撤销使用以下个人接口：

- `GET /api/v1/auth/sessions` 查询当前租户用户的有效设备；只要求 Bearer token。
- `DELETE /api/v1/auth/sessions/{sid}` 撤销一个设备；要求 Bearer token、challenge Cookie 和
  `X-CSRF-Token`。撤销当前 `sid` 后认证 Cookie 必须被清除，后续 Bearer 请求应返回 `401`。
- `POST /api/v1/auth/sessions/revoke-others` 保留当前设备并撤销其他设备；CSRF 要求与单设备
  撤销相同，响应 `revoked_count` 只统计本次实际撤销数量。

单设备目标不存在、跨租户或属于其他用户时统一返回 `404`；CSRF 失败为 `403`；Redis 或会话
索引不可用为 `503`。排障时先使用 API 返回的 `request_id` 串联日志，不要记录完整 SID 列表，
更不得记录 access token、refresh token 或 CSRF challenge。

每个租户用户最多允许 256 个活跃设备会话。达到上限后新登录应返回 `409`，先让用户从仍可用的
设备撤销旧会话，不能通过增大 Redis 脚本输入或直接编辑索引绕过限制。正常批量撤销最多处理
256 个候选；若升级遗留、损坏或人工写入的索引超过该边界而返回 `400`，按
`GET /api/v1/auth/sessions` 的结果逐一调用 `DELETE /api/v1/auth/sessions/{sid}`，每次确认成功后
再处理下一条。逐一撤销是有审计和身份校验的安全降级路径，直接 `DEL` Refresh Family、租户索引
或在线用户 metadata 会破坏事实与索引的一致性。

Refresh Family 是唯一事实来源，在线用户 metadata 只保存浏览器、IP 等展示信息。版本升级后
至少七天内同时读取新租户/用户索引与升级前 metadata，并逐条回查 Family；这覆盖当前最大
Refresh TTL，保证旧会话可查看和撤销。兼容窗口内不要提前移除旧键、关闭双读或用全量 Redis
清理作为发布步骤。多实例验收必须分别从两个 API 实例登录、列出并撤销同一用户会话，确认两边
立即得到一致结果；内存模式只做单进程验收，不能证明生产一致性。

### 限流拒绝

按 `scope` 判断是攻击、客户端重试风暴还是正常容量不足。恶意来源在边缘封禁；客户端
问题要求指数退避并遵守 `Retry-After`。只有证明正常峰值确实超过基线且下游有余量后，
才能调整限流参数。

### 服务账号、Agent 凭据和访问审计

Agent 调用异常先按 request ID 查询 `GET /api/v1/system/service-access-audits`，核对
`operation_id`、`capability_key`、`access_mode`、`result`、`reason_code`、HTTP 状态、行数、响应
字节和完成时间；该接口需要 `system:service-access-audit:list`。审计不包含 API Key、委托令牌、
响应正文、原始 IP 或原始 user-agent，禁止为排障把这些敏感值补写到操作日志、工单或聊天记录。
原始凭据一旦泄露只能撤销并轮换，不能从数据库、审计或幂等响应恢复。

常见处置顺序如下：

1. `401 invalid_credential`：确认请求使用精确的
   `Authorization: RyFrameApiKey rfk_<key_id>.<secret>`，委托模式另有
   `X-RyFrame-Delegation: rfd_<secret>`；再检查账号、Key、委托、用户状态与数据库 UTC 到期时间。
   响应故意不说明 Key ID、Secret、Pepper 或委托哪一项失败，不能通过扩大日志泄露判定细节。
2. `403`：从服务账号详情核对角色，使用个人委托能力接口重新计算双方共同能力，并检查委托白名单。
   用户/部门查询还受双方数据范围交集约束；岗位/字典要求双方范围都是全部。不要临时授予超级角色、
   手改关系表或绕过数据范围验证问题。
3. `429`：保留 `Retry-After`，区分预认证 IP、租户、账号、凭据、被代表用户、账号+能力和账号并发
   七个维度。先排查重试风暴、重复并发与调用方是否复用已撤销 Key，再依据容量证据调整
   `APP_SERVICE_ACCOUNTS_DEFAULT_REQUESTS_PER_MINUTE`、账号请求上限或并发上限；不得直接删除 Redis
   键来解除生产告警。
4. `413`：缩小页大小或响应范围，不能提高到超过 1 MiB 的硬上限。`503 timeout` 先检查慢 SQL、锁
   等待和主库连接，再评估 100–30000 ms 范围内的查询时限。依赖或审计不可用产生的 `503` 必须先
   修复 MySQL/Redis，不得切换副本或关闭审计放行。
5. 未注册 Agent 路径返回审计后的 `404`。持续出现时检查调用方版本、OpenAPI 的
   `x-ryframe-agent-capabilities` 和反向代理改写；Agent 不支持客户端自定义 operation、过滤或排序。

Key 轮换采用“双 Key”窗口：携带新的 `Idempotency-Key` 创建第二把 Key，立即安全保存首次响应中的
完整 Key，切换调用方并验证审计后撤销旧 Key。默认每账号最多两把有效 Key且每把最长 90 天；幂等
重放只返回元数据，`secret=null`。如果首次响应丢失，不得继续重放期待取回 Secret，应撤销该凭据后
重新创建。委托 Token 同样只显示一次；默认 24 小时、最长 30 天，撤销后不可恢复。

Pepper 轮换必须先把新版本加入外部 Keyring，保留仍被有效 Key/委托引用的旧版本，把
`APP_SERVICE_ACCOUNTS_ACTIVE_PEPPER_VERSION` 指向新版本，再滚动重启 API。确认新建凭据已使用新
版本、旧凭据仍可按预期校验后，等待旧凭据与委托全部到期或撤销，才能从 Keyring 移除旧 Pepper。
Keyring 解析、权限、活动版本或挂载故障会阻止服务启动；不得回退复用 JWT Secret、把 Pepper 写入
TOML/环境变量/日志，或在滚动期间让实例使用不相容的版本集合。

服务访问审计默认保留 180 天并由既有数据保留任务清理，没有新增 Worker。出现增长异常时先按
成功、拒绝、未知路径和依赖错误拆分调用量，再检查预认证攻击和客户端重试。只能通过数据保留预览
与受控任务清理到期行；不得手工删除当前保留窗口中的审计，也不得把降低保留期作为限流替代方案。

### 数据库副本与读回退

先核对 `ryframe_db_node_up` 的副本名称、数据库连接日志和复制延迟，再从应用网络执行
只读探针。副本监督器每 5 秒运行一次，单次连接/PING/结构校验总超时为 2 秒；网络失败
连续三次才摘除，结构指纹不一致会立即摘除，并按 5–60 秒退避重试。不要绕过健康阈值
强制恢复副本；在副本恢复前应确认主库连接容量能够承受回退流量。读回退比例升高时，同时
检查副本摘除、连接池耗尽和最近的网络或数据库变更。

### 后台任务

先按 `type` 确认最早可执行任务的等待时长、Worker 存活状态和租约是否持续推进。死信
任务必须检查 `last_error`、关联业务数据和幂等性后才能人工重试；不要通过直接修改任务
状态或确认时间来清除告警。

MySQL 始终是后台任务和 Outbox 的唯一可靠事实来源。`ryframe:jobs:wakeup` Redis Pub/Sub
频道以及进程内通知只用于提前结束等待；提示丢失、重复、未知负载或 Redis 订阅中断时，Worker
仍会经数据库轮询领取任务，不能把 `job_wakeup_listener_up=0` 直接判定为任务丢失。订阅会按
1、2、4、8、16、30 秒退避重连；首次异常为 WARN，持续异常为 DEBUG，恢复为 INFO。

`APP_JOBS_POLL_INTERVAL_MS` 是最小空闲等待，而不是固定轮询间隔。连续空闲以 2 倍增长并在
`APP_JOBS_MAX_IDLE_POLL_INTERVAL_MS` 封顶，每次等待附加固定 ±20% 抖动；领取任务、收到唤醒和
人工重试都会立即重置。基础设施错误继续使用独立的最长 30 秒错误退避。过期租约恢复不跟随
空闲等待，而是按 `APP_JOBS_LEASE_RECOVERY_INTERVAL_SECONDS` 独立运行；调整任一 `APP_JOBS_*`
值后必须同时重启 API 与 Worker。

### 定时调度

Cron 调度同样只以 MySQL 为可靠事实来源。`jobs.mode=embedded` 时由 API 扫描，
`jobs.mode=external` 时只由 `ryframe-worker` 扫描，`disabled` 不扫描；`ryframe-worker`
仅接受 `jobs.mode=external`，包括 `--once`。`APP_JOBS_SCHEDULER_ENABLED=false` 只关闭计划管理、
路由、菜单与扫描，不停止普通后台任务、Outbox、重试或死信处理。修改该开关后必须同时重启 API
和 Worker。关闭期间已经入队的计划任务会继续完成；重新启用后按每条计划的 `skip` 或
`fire_once` 策略处理错过的执行，不会逐条补跑全部历史。

`ryframe-worker --once` 在调度启用时会先扫描一批到期计划，
再执行单次 Outbox 和任务消费。扫描间隔、批量大小和单租户启用上限
分别由 `APP_JOBS_SCHEDULER_POLL_INTERVAL_MS`、`APP_JOBS_SCHEDULER_BATCH_SIZE` 和
`APP_JOBS_MAX_ENABLED_SCHEDULES_PER_TENANT` 控制。多实例通过 `FOR UPDATE SKIP LOCKED` 与
`schedule_id + fire_key` 唯一键去重，禁止把 Redis 唤醒是否成功当作计划是否触发的判断依据。

计划时间统一以 UTC 保存，Cron 使用“秒 分 时 日 月 周 年”七段格式，秒字段只允许 `0`，
年字段只允许 `*`，时区必须是 IANA 名称。日期字段与星期字段不能同时受限，其中一项必须为
`*`。管理端优先使用可视化规则生成器并核对服务端返回的未来五次时间；高级表达式也必须先通过
预览。夏令时开始时不存在的本地时间会跳过，夏令时结束时重复的本地时间可能对应两个不同 UTC
时刻，应在计划时区、本地时区和 UTC 三列中逐项确认。排障时结合
`job_schedule_scan_total{result}`、`job_schedule_trigger_total{outcome}` 与
`job_schedule_lag_seconds`；这些指标不允许增加租户、计划、任务、表达式或错误文本标签。

数据库中被手工破坏的 Cron、时区、目标或租户范围、错过策略、并发策略或最大运行时长会写入
`invalid_configuration` 执行历史，随后自动禁用并清空下次执行时间。数据库连接或入队等瞬时
错误不会禁用计划，应先排查基础设施并等待下一轮扫描重试。

如需在后续版本完整移除 Cron，先设置 `APP_JOBS_SCHEDULER_ENABLED=false`，同时重启 API 和
Worker，确认定时任务菜单与接口不可用、扫描指标停止增长，并观察至少 24 小时确认普通后台任务
仍正常。随后可删除调度服务、目标注册表、调度 API、前端页面、可视化生成器、调度配置、指标、
告警和旧每日调度兼容模块。数据库历史默认保留；确需删除时只能新增前向迁移，不能修改旧迁移。
`JobQueue`、`JobWorker`、Outbox、导出、消息、租约、重试和死信必须保留。

排障时结合 `job_claim_attempts_total{queue,result}`、
`job_wakeup_total{queue,transport,result}`、`job_wakeup_listener_up{queue}`、
`job_wakeup_protocol_errors_total{result}` 观察领取、唤醒和协议错误；这些标签均为固定低基数枚举。
授权缓存使用 `authorization_cache_lookups_total{scope,result}`，其中 scope 仅为
`snapshot`、`tenant`、`namespace`，result 仅为 `hit`、`miss`、`bypass`、`fallback`、`error`。不得
向任一指标标签添加租户、用户、任务 ID 或 Redis 错误文本。

### 数据保留

数据保留是永久硬删除。默认系统计划每天 `03:30 UTC` 入队 `system.data_retention.cleanup`；
关闭 Cron 时自动运行停止，但系统租户的人工运行入口仍可用。首次启用或提高删除范围前，必须先在
“数据保留”页面执行预览，记录数据库计算时间、策略快照、各资源截止时间和预计数量，并由值班人员
与数据负责人共同确认。

运行状态为 `partial` 表示某个资源达到单次上限，不是数据错误；下一次运行会继续。状态为 `failed`
时，先在后台任务页核对重试和死信，再查看运行记录中的安全错误摘要。已经提交的资源批次不会因后续
资源失败而回滚，不得手工把运行状态改为成功。任何时候都不能为释放空间直接删除 `pending`、
`running`、`dead` 后台任务或未成功发布的 Outbox。导入源文件和报告必须通过 FileService 清理对象与
元数据，不能只删数据库记录或底层对象。

保留任务持续 `partial` 时，先确认 `APP_DATA_RETENTION_MAX_ROWS_PER_RESOURCE_PER_RUN` 与运行频率是否
覆盖每日增长，再评估提高单次上限或缩短计划周期。调整前检查 MySQL 复制延迟、锁等待、连接池和磁盘
吞吐，不能用无限增大批次掩盖缺失索引。

### 异步用户导入

用户导入任务由普通后台任务 Worker 消费。等待任务长期不运行时，确认部署顺序是否为“迁移 → 新
Worker → API → 前端”，并验证当前 Worker 已注册 `system.user.import`。任务载荷中只能出现导入任务
ID；发现 Excel 行、密码、用户资料或对象存储地址进入日志、审计或任务公开视图时按安全事件处置。

运行中进度只在批次提交后前进。Worker 中断、租约丢失或重试后应从 `processed_rows` 继续；进度回退、
同一行被重复创建或租户配额被批次绕过属于故障。取消只在批次边界生效，已经提交的用户不会回滚。
申请人被停用、撤权或失去部门数据范围时，后续批次应停止并保留已提交结果。

导入模板只允许 `用户名`、`昵称`、`邮箱`、`手机号`、`部门完整路径` 五列。遇到旧版 `部门ID`、缺列、未知列、重复表头或顺序变化时，应让用户重新下载模板，不能手工把数据库 ID 填回文件。部门路径来自模板的“可用部门”工作表；路径无匹配、重复匹配、部门停用、层级损坏或越权时必须安全失败，不得回退为叶子名称或任意选择第一条记录。

报告未就绪返回 `409`，没有跳过或失败行返回 `404`；不能把这两种状态当作对象丢失。源文件或报告超过
保留窗口后，页面应明确显示已过期。对象存储不可用时创建导入必须返回 `503`，不得把文件复制到临时
目录绕过私有 bucket 和清理策略。

### 权限诊断与运维总览

权限诊断只读主库结果。缓存状态为 `stale`、`missing` 或 `unavailable` 时，先比较数据库租户授权纪元、
用户授权版本和缓存版本，再检查 Redis 与授权通知链路；诊断成功不代表目标用户当前 WebSocket 一定
在线。禁止通过数据库手工改纪元、伪造缓存或临时授予更高权限来“验证”问题。

运维总览严格按当前租户统计。系统租户只额外包含无租户的平台后台任务，不能把其他普通租户的数据
并入图表。Redis、对象存储或消息中心显示降级时，快照接口仍可成功；主库不可用时应返回 `503`。趋势
图为空时先核对时间范围和租户活动，再检查对应时间索引与 SQL 执行计划，不要用 Prometheus 数据手工
回填 MySQL 趋势结果。

### 平台租户容量与配额

平台租户容量查询只从 `system` 租户发起。原有 `GET /api/v1/platform/tenants` 是兼容基础列表；分页、
详情和独立用量分别使用 `/page`、`/{tenant_id}`、`/{tenant_id}/usage`。基础列表与分页要求
`tenant:list`，用量要求 `tenant:usage:list`；容量筛选也要求后一个权限。普通租户访问返回 `403`，
不能为了排障把该权限复制到普通租户；迁移会拒绝普通租户中的保留权限代码碰撞，以及迁移前已经存在的
普通角色绑定。系统租户可在迁移完成后按职责把该权限显式授予平台管理角色，迁移本身不会自动授权。

容量状态异常时先核对主库中的租户配额、未软删除用户、未软删除角色和未软删除文件元数据。`0`
配额表示无限制，不表示禁止使用；不要把它改成极小正数来“恢复”接口。状态阈值为 80%、90%、
100%，整体状态取用户、角色和存储中最严重的一项。对象存储目录可能存在清理中对象或暂存对象，
不能用目录扫描覆盖 `sys_file` 的权威统计。

分页或详情变慢时，使用代表性绑定值检查分页计数、容量状态筛选和当前页聚合的执行计划，确认命中
`idx_user_tenant_del`、`idx_role_tenant_del`、`idx_file_tenant_del_size`、
`idx_schedule_tenant_del_enabled`。同一页的 SQL 次数应保持有界，不随租户数线性增加；发现逐租户
计数时按 N+1 回归处理，不能用缩小默认页大小掩盖问题。

请求卡片只代表当前一分钟限流窗口。其 `status=unknown` 时检查 Redis 连通性、ACL/TLS、事务执行与键
过期时间；用户、角色、存储和辅助汇总仍正常即属于预期局部降级，不应把整页判定为失败。不要根据
当前窗口推导历史请求量，也不要通过手工写 Redis 计数修正页面。页面与客户端不得后台持续轮询。
Prometheus 指标只能使用固定低基数标签，禁止加入租户 ID、租户名称或用量状态对应的资源标识。

### 租户配置包迁移

配置迁移任务长期等待时，先确认发布顺序为“迁移 → 已注册配置迁移任务的新 Worker → API → 前端”，
再检查普通后台任务领取、租约和死信。配置包生成、上传、预览、应用和回滚的业务状态以 MySQL 为
准；Redis 只负责唤醒，不能通过重发 Redis 消息修正业务状态。任务载荷只能包含配置包或迁移记录 ID，
若发现包内容、对象路径、Secret 或配置值进入任务公开视图、日志或审计参数，应按安全事件处置。

上传被拒绝时核对默认 5 MiB 压缩、20 MiB 解压和 10,000 项限制，以及 ZIP 是否严格只有根目录
`manifest.json`、`resources.json`。不要通过放宽路径、压缩比、JSON 深度、SHA-256 或稳定键校验来
“修复”第三方配置包；应回到来源租户重新生成。对象存储中的配置包和快照只能位于私有
`config-packages` bucket，不得临时复制到公开 bucket 或主机临时目录。

应用返回 `409` 时，先比较预览记录的 `plan_hash`、`configuration_version`、
`authorization_epoch` 与目标主库当前值；任一变化都要求重新预览。普通部门、岗位、字典、参数、
权限、菜单、角色、套餐或数据放置写入返回 `409 tenant_operation_conflict` 时，检查
`sys_tenant_operation_lease` 的所有者、操作和数据库到期时间。
不得手工删除仍有效租约；确认任务已经终止且租约过期后，由正常接管流程处理。排查死锁时核对统一
锁序是否为“租户行 → 配置租约 → 资源/关系行”。

应用前快照上传失败意味着数据库不应发生任何配置变化；若数据库事务失败，所有资源写入必须整体
回滚。回滚默认只在应用成功后 168 小时内可请求，且会拒绝版本漂移、后续人工修改、新引用或快照
缺失。配置包对象默认保留 168 小时；过期清理由 FileService 同时处理对象和 `sys_file` 元数据，
不能只删一侧。配置包和迁移历史元数据目前没有已配置的自动硬删除期限，不要以对象过期为由手工
删除审计记录。

### 消息中心投递

区分慢消费者与已关闭连接：前者检查客户端消费、网络和连接数，后者确认是否为正常断线
重连。新连接必须先收到 hello，服务端才能把该连接标记为可投递并触发收件箱补拉；消息先于
hello 表示协议顺序异常。持久化收件箱在 ACK 落库前提供至少一次投递，因此客户端必须按
message ID 做逻辑合并，ACK 前出现同 ID 的多个原始帧本身不等于消息重复入账。服务端确认
ACK 已持久化后，新连接跨完整补拉周期仍收到该 ID 才属于故障。排障期间不得手工写入
`acked_at`，以免掩盖未送达消息。消息中心运行边界统一来自 `[messaging]`；可使用 `APP_MESSAGING_ENABLED`、
`APP_MESSAGING_TICKET_TTL_SECONDS`、`APP_MESSAGING_RETENTION_DAYS`、
`APP_MESSAGING_MAX_CONNECTIONS_PER_USER`、`APP_MESSAGING_OUTBOUND_BUFFER` 和
`APP_MESSAGING_MAX_RECIPIENTS_PER_MESSAGE` 覆盖；共享补拉使用
`APP_MESSAGING_REPLAY_INTERVAL_SECONDS`、`APP_MESSAGING_REPLAY_JITTER_SECONDS` 和
`APP_MESSAGING_REPLAY_BATCH_SIZE`。修改后必须重启 API 与 Worker。

`acked_at` 是接收后自动写入的送达时间，`read_at` 仅在用户打开详情后写入，已读记录必须同时
已送达。`deleted_at` 仅软删除当前收件人的记录，不删除主消息或其他用户的收件箱；不要通过
数据库手工修改这三个字段。用户批量删除走 `POST /api/v1/system/messages/delete`，每次最多 100
个字符串 ID；重复调用预期返回 0，不应被当成失败。删除后仍出现在未读数、补拉或 WebSocket
重放中，才属于需要排查的隔离或查询条件故障。

达到每用户连接上限时先排查客户端重复建连和退避策略，不要直接放大容量；慢消费者会以
WebSocket `1013` 关闭，连接数超限使用策略关闭。发布被收件人数上限拒绝时，事务不会留下
消息或部分收件箱快照；应缩小受众、拆分业务消息或在完成容量评估后调整上限。生产启用
消息中心时 Redis 必须为 `required`，关闭消息中心会同时停止票据、实时订阅和消息任务，
不能把它当作仅关闭 WebSocket 的开关。

`ryframe_message_replay_query_total{result="success|error"}` 只按有界结果记录共享补拉查询。
同一租户用户建立多个连接时，查询增量应按身份而非连接数增长；异常放大通常表示客户端
反复建连、周期过短或共享调度器退化，不能靠继续增大数据库连接池掩盖。

### API 与 Worker 优雅关闭

API 和独立 Worker 的全局关闭宽限均为 5 秒。Unix 进程支持 Ctrl+C 和 SIGTERM；Windows
控制台进程支持 Ctrl+C 和 Ctrl+Break。Windows 服务包装器或发布脚本应先发送控制台关闭
事件并等待进程自行退出，不得用 `taskkill /F` 的结果冒充优雅关闭；只有超过平台总宽限后才
允许执行强制终止。正常关闭必须同时满足退出码为 0、出现应用关闭日志，并且从信号送达到
进程完全退出不超过 5 秒。

2026-08-07 在隔离数据库、Redis 禁用和本地对象存储环境中，以独立 Windows 进程组发送真实
`CTRL_BREAK_EVENT` 完成验收：包含内置 Job Worker、Outbox、消息保留、就绪探测和服务器采样器
的 API 在 989.725 ms 内退出；独立 Worker 连同健康服务在 69.703 ms 内退出。两个进程退出码均
为 0，并分别写出关闭信号和停止完成日志。该验收不能由 `Stop-Process` 或直接调用内部关闭函数
替代。

### OpenTelemetry 导出器

检查 OTLP endpoint 的 DNS、TLS、认证和网络出口策略，并确认服务配置中的采样与超时值。
初始化失败会触发 `ryframe_otel_exporter_degraded`；运行或关闭期间的失败会累计在
`ryframe_otel_exporter_runtime_failures_total`。两类告警均不应阻断就绪探针；恢复后验证
新的 trace 能抵达后端，再关闭告警。

OTel 故障日志只使用固定的 `failure_stage=initialization|export|shutdown`，指标不携带 endpoint、
租户、用户或请求标识等动态标签，避免泄露连接信息并控制基数。退出时 flush 的总等待上限固定
为 5 秒；应在受控环境验证导出失败前后业务与 `/readyz` 均保持可用、运行期计数递增且关闭
不会越过时限。

后台任务和 Outbox 会同时持久化 W3C `traceparent` 与 `tracestate`，Worker 领取后恢复为远端父
上下文。排查链路断裂时应同时核对 `sys_background_job`、`sys_outbox_event` 的两列与 Collector
收到的父子关系；不得只复制 `traceparent` 后手工执行任务，否则会丢失供应商链路状态。

### 应用日志输出与保留

API 与独立 Worker 使用相同的日志配置。生产容器默认
`APP_LOGGER_LEVEL=info`、`APP_LOGGER_FORMAT=json`、`APP_LOGGER_OUTPUT=stdout`，本地不创建
`logs/`；日志轮转、保留和容量告警由容器平台统一负责。先检查平台采集器、实例标签和
保留策略，再排查应用日志级别，避免把采集故障误判为应用未记录日志。

非容器部署需要本地文件时，可显式设置 `APP_LOGGER_OUTPUT=file`。应用会在工作目录的
`logs/` 中按日滚动 `ryframe.log.*`，并按 `APP_LOGGER_RETENTION_DAYS` 保留 1–3650 个最近
文件；API 和 Worker 都必须拥有目录写权限。容器内启用文件模式前必须挂载可写持久卷，
否则只读根文件系统会使进程拒绝启动。修改任一 `APP_LOGGER_*` 配置后需重启对应进程；
排障时不得手工删除仍在保留窗口内的日志。

数据库 SQL 日志由 `APP_DATABASE_SQL_LOG_LEVEL` 和
`APP_DATABASE_SQL_SLOW_THRESHOLD_MS` 控制。常态保持 `off`；`slow` 只输出达到阈值的 WARN，
`summary` 输出所有摘要，`full` 输出完整参数化 SQL。临时启用 `summary` 或 `full` 前记录负责
人和停止时间，恢复后立即切回 `off`。每条 SQL 只应生成一条最终记录；日志可关联请求、租户、
用户或任务，但不得出现绑定参数值、密码、令牌或连接串。连续空轮询、正常心跳、健康成功和
无工作量清理是 DEBUG，不应在 INFO/WARN 持续刷屏。

### 磁盘容量

检查应用本地卷、MySQL、Redis、对象存储和日志节点。优先清理有保留策略且可重建的
临时文件或过期日志；数据库、AOF、对象和最近有效备份不得直接删除。低于 5% 时停止
非关键写入并扩容。恢复后验证 inode、备份和对象读写。

### 备份失败或过期

检查备份任务、目标容量、凭据和网络。立即使用部署环境自有工具补做备份、摘要校验和隔离库恢复演练；只有三步全部成功才更新成功时间指标。不得用未经恢复验证的文件解除告警。

RyFrame 不创建或恢复 MySQL 备份。数据库平台完成备份后，用
`ryframe-tenant-data backup-register` 登记不透明 provider reference、捕获时间、Schema 指纹和
校验摘要，同时必须传入数据库平台真实的 `--retention-until`，有明确过期时间时还要传
`--expires-at`；不得由应用或值班人员虚构保留期限。执行登记即确认平台侧摘要和恢复校验已完成。
专属目标可登记租户级整库恢复点；共享目标只能登记分片级恢复点，不能把整库备份描述成
可直接恢复单租户。完整租户灾备必须同时包含控制库、业务数据目标和对象存储备份。

### TLS 证书

确认告警对应的实际 SNI、证书链和终止层。续期后从外部网络验证管理端与 API 域名，
检查完整链、OCSP/系统信任和到期时间，再重新加载 Nginx。不要通过关闭证书校验绕过。

## 5. 发布观察面板

每次稳定版发布至少展示：请求率、5xx 比例、P50/P95/P99、in-flight、进程 CPU/内存、
MySQL 连接比例、Redis 降级、限流拒绝、refresh 重放、磁盘余量、最近备份成功时间和
证书剩余天数。发布记录应链接长期保留的面板时间窗，并标注版本 SHA。
