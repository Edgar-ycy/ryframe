# 运维

## 部署与发布

生产镜像只包含 API、迁移和 Worker 二进制，不包含生成器、文件维护或 reset。基础镜像、CI Action 和工具镜像必须固定完整摘要。

发布前必须完成镜像构建、Compose 展开、Nginx 校验、Prometheus 规则校验、SBOM、漏洞和 license 检查。前后端使用相同版本号和不可移动的同名 tag；先推前端 tag，再推后端 tag。

生产升级不得使用破坏性重建。当前版本只接受全新生产库；已有真实生产旧库需要另行设计非破坏升级方案。

## 非生产重建

`ryframe-reset` 只在显式 `destructive-reset` feature 下编译，生产镜像不复制该二进制。执行前必须停止 API、Worker 和 scheduler，并完成全部只读预检。

```powershell
$env:APP_ENV = "test"
cargo run --locked -p ryframe --features destructive-reset --bin ryframe-reset -- plan
cargo run --locked -p ryframe --features destructive-reset --bin ryframe-reset -- execute --plan-hash <sha256> --confirm-reset <精确短语>
```

执行顺序固定为对象前缀、Redis namespace、物理数据库、控制 baseline、租户 baseline、验证。清单、ledger 和 report 不包含秘密；失败后只允许使用同一清单续跑。生产环境在读取配置或访问外部资源前永久拒绝。

## HTTP 5xx 与高延迟

先按请求 ID 检查 API 日志和 trace，再检查数据库、Redis、对象存储与 Worker。确认是单一依赖故障后按降级策略处理；不要通过扩大重试放大故障。

## Redis 降级

检查连接模式、TLS、ownership marker 和 scope 前缀。授权缓存不可用时必须失败关闭或按配置禁用缓存；禁止清空共享 Redis DB。

## Refresh Token 重放

立即撤销 token family 和当前会话，核对用户授权版本、客户端 IP 与审计事件。确认泄露后轮换相关凭据并保留证据。

## 限流拒绝

区分租户配额、账号策略和基础设施故障。只在确认业务容量允许后调整配置，不删除限流键或绕过租户边界。

## 数据库副本与读回退

检查复制延迟、连接健康和 read consistency。授权、任务、导出与状态变更必须回到主库；不要把强一致读临时改为副本读。

## 后台任务

检查 lease、heartbeat、attempt、公开错误和死信状态。重复领取必须由数据库门禁拒绝；修复原因后使用受控重试，不直接改任务终态。

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

停止依赖无效备份的迁移操作，检查目标、校验和、完成时间和保留策略。恢复后重新生成并验证备份，不手工标记为成功。

## TLS 证书

检查到期时间、主机名、完整链和私钥权限。轮换后验证 MySQL、Redis、对象存储、OTLP 与 Nginx；不要关闭证书校验作为长期修复。

## 故障记录

记录时间线、影响租户、请求或任务 ID、根因、处置和验证结果。配置字段以配置结构为准，指标和告警以部署资产为准，不在本文维护重复清单。
