# 生产监控与值班手册

> 最后核对：2026-07-25

本文档定义 RyFrame 生产环境的最低监控、告警和处置要求。告警规则模板位于
`deploy/prometheus/ryframe-alerts.yml`。模板中的阈值是初始值，上线后应依据容量测试
和至少两周的生产基线调整，但不得在没有替代保护的情况下直接删除告警。

## 1. 生产暴露边界

- `APP_API_DOCS_ENABLED=false` 是生产强制配置；Nginx 还会对 Swagger UI 和 OpenAPI
  JSON 返回 `404`。需要排查契约时使用仓库中的 `openapi/openapi.json`，不要临时对公网
  开启运行时文档。
- `APP_MONITOR_METRICS_BEARER_TOKEN` 必须由密钥管理系统注入，至少 32 字节，独立于
  用户 JWT 密钥并定期轮换。Nginx 仅允许 Prometheus/VPN 网段访问 metrics 路径，应用
  再校验 Bearer Token。
- `/livez` 可用于进程存活探测；`/readyz` 会检查关键依赖。公网负载均衡器仅需访问
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
```

轮换 Token 时按实例分批：Prometheus 临时建立两个使用不同 secret、且分别直连新旧
实例池的 scrape job；更新一批实例后确认新 Token 抓取成功，再更新剩余实例并删除旧
job。不能让两个 Token 经同一个随机负载均衡目标抓取，否则会产生间歇性 `401`。任何
时候都不能把 Token 写入 URL。

## 2. 采集依赖

| 能力 | 数据源 |
| --- | --- |
| HTTP 错误率、P95/P99、Redis 降级、refresh 重放、限流 | RyFrame `/api/v1/monitor/metrics` |
| MySQL 连接容量 | `prometheus/mysqld_exporter` |
| 主机与存储卷磁盘 | `prometheus/node_exporter` |
| TLS 到期 | `prometheus/blackbox_exporter` HTTPS probe |
| 备份成功时间 | 备份任务通过 node exporter textfile collector 或 Pushgateway 发布 |

备份指标必须只在 `deploy.sh validate` 和恢复演练成功后更新：

```text
# HELP ryframe_backup_last_success_timestamp_seconds Last validated backup Unix timestamp.
# TYPE ryframe_backup_last_success_timestamp_seconds gauge
ryframe_backup_last_success_timestamp_seconds 1784937600
```

如果未部署上述 exporter，对应告警不是“已覆盖”。上线清单必须记录每条规则的
Prometheus 查询结果和 Alertmanager 测试通知。

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

### 磁盘容量

检查应用本地卷、MySQL、Redis、对象存储和日志节点。优先清理有保留策略且可重建的
临时文件或过期日志；数据库、AOF、对象和最近有效备份不得直接删除。低于 5% 时停止
非关键写入并扩容。恢复后验证 inode、备份和对象读写。

### 备份失败或过期

检查备份任务、目标容量、凭据和网络。立即补跑 `backup`、`validate` 和隔离库
`rehearse`；只有三步全部成功才更新成功时间指标。不得用未经恢复验证的文件解除告警。

### TLS 证书

确认告警对应的实际 SNI、证书链和终止层。续期后从外部网络验证管理端与 API 域名，
检查完整链、OCSP/系统信任和到期时间，再重新加载 Nginx。不要通过关闭证书校验绕过。

## 5. 发布观察面板

每次 RC/stable 至少展示：请求率、5xx 比例、P50/P95/P99、in-flight、进程 CPU/内存、
MySQL 连接比例、Redis 降级、限流拒绝、refresh 重放、磁盘余量、最近备份成功时间和
证书剩余天数。发布记录应链接长期保留的面板时间窗，并标注版本 SHA。
