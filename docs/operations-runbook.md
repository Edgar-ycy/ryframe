# 生产监控与值班手册

> 最后核对：2026-07-25

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

### Refresh Token 重放

按安全事件处理。保全相关请求 ID、账号、租户、来源 IP 和 user-agent；确认 token family
已被吊销，必要时吊销该用户全部会话并通知安全负责人。禁止在工单或聊天中复制原始
Token。排查日志泄漏、代理查询参数、浏览器插件和客户端并发刷新。

### 限流拒绝

按 `scope` 判断是攻击、客户端重试风暴还是正常容量不足。恶意来源在边缘封禁；客户端
问题要求指数退避并遵守 `Retry-After`。只有证明正常峰值确实超过基线且下游有余量后，
才能调整限流参数。

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

### 消息中心投递

区分慢消费者与已关闭连接：前者检查客户端消费、网络和连接数，后者确认是否为正常断线
重连。持久化收件箱会补拉未确认消息，排障期间不得手工写入 `acked_at`，以免掩盖未送达
消息。消息中心运行边界统一来自 `[messaging]`；可使用 `APP_MESSAGING_ENABLED`、
`APP_MESSAGING_TICKET_TTL_SECONDS`、`APP_MESSAGING_RETENTION_DAYS`、
`APP_MESSAGING_MAX_CONNECTIONS_PER_USER`、`APP_MESSAGING_OUTBOUND_BUFFER` 和
`APP_MESSAGING_MAX_RECIPIENTS_PER_MESSAGE` 覆盖；共享补拉使用
`APP_MESSAGING_REPLAY_INTERVAL_SECONDS`、`APP_MESSAGING_REPLAY_JITTER_SECONDS` 和
`APP_MESSAGING_REPLAY_BATCH_SIZE`。修改后必须重启 API 与 Worker。

达到每用户连接上限时先排查客户端重复建连和退避策略，不要直接放大容量；慢消费者会以
WebSocket `1013` 关闭，连接数超限使用策略关闭。发布被收件人数上限拒绝时，事务不会留下
消息或部分收件箱快照；应缩小受众、拆分业务消息或在完成容量评估后调整上限。生产启用
消息中心时 Redis 必须为 `required`，关闭消息中心会同时停止票据、实时订阅和消息任务，
不能把它当作仅关闭 WebSocket 的开关。

`ryframe_message_replay_query_total{result="success|error"}` 只按有界结果记录共享补拉查询。
同一租户用户建立多个连接时，查询增量应按身份而非连接数增长；异常放大通常表示客户端
反复建连、周期过短或共享调度器退化，不能靠继续增大数据库连接池掩盖。

### OpenTelemetry 导出器

检查 OTLP endpoint 的 DNS、TLS、认证和网络出口策略，并确认服务配置中的采样与超时值。
初始化失败会触发 `ryframe_otel_exporter_degraded`；运行或关闭期间的失败会累计在
`ryframe_otel_exporter_runtime_failures_total`。两类告警均不应阻断就绪探针；恢复后验证
新的 trace 能抵达后端，再关闭告警。

OTel 故障日志只使用固定的 `failure_stage=initialization|export|shutdown`，指标不携带 endpoint、
租户、用户或请求标识等动态标签，避免泄露连接信息并控制基数。退出时 flush 的总等待上限固定
为 5 秒；本地可运行 `cargo test -p ryframe-middleware telemetry`，使用不响应的回环地址验证导出
失败前后业务与 `/readyz` 均保持可用、运行期计数递增且关闭不会越过时限。

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

### 磁盘容量

检查应用本地卷、MySQL、Redis、对象存储和日志节点。优先清理有保留策略且可重建的
临时文件或过期日志；数据库、AOF、对象和最近有效备份不得直接删除。低于 5% 时停止
非关键写入并扩容。恢复后验证 inode、备份和对象读写。

### 备份失败或过期

检查备份任务、目标容量、凭据和网络。立即使用部署环境自有工具补做备份、摘要校验和隔离库恢复演练；只有三步全部成功才更新成功时间指标。不得用未经恢复验证的文件解除告警。

### TLS 证书

确认告警对应的实际 SNI、证书链和终止层。续期后从外部网络验证管理端与 API 域名，
检查完整链、OCSP/系统信任和到期时间，再重新加载 Nginx。不要通过关闭证书校验绕过。

## 5. 发布观察面板

每次稳定版发布至少展示：请求率、5xx 比例、P50/P95/P99、in-flight、进程 CPU/内存、
MySQL 连接比例、Redis 降级、限流拒绝、refresh 重放、磁盘余量、最近备份成功时间和
证书剩余天数。发布记录应链接长期保留的面板时间窗，并标注版本 SHA。
