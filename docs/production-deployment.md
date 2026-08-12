# 生产部署基线

> 最后核对：2026-08-12

本文档给出 RyFrame 生产环境的最低安全和可靠性基线。示例配置必须按实际域名、网段、
证书和容量修改，不能原样作为生产凭据或网络策略。

## 1. 镜像构建与供应链

`deploy/Dockerfile` 使用显式 Rust patch 版本和 Debian major 版本，并通过 BuildKit
缓存 Cargo registry、git checkout 和 `target`。缓存只影响构建速度，最终二进制会复制
到普通镜像层，不依赖构建机缓存。Dockerfile 的生产构建固定使用
`--no-default-features`，因此 API 镜像不包含只用于开发/受控测试的 Swagger UI 静态资源：

```bash
DOCKER_BUILDKIT=1 docker build \
  --file deploy/Dockerfile \
  --build-arg RYFRAME_BUILD_COMMIT="$(git rev-parse HEAD)" \
  --tag registry.example.com/ryframe:"$(git rev-parse --short HEAD)" .
```

`RYFRAME_BUILD_COMMIT` 必须是小写的完整 40 位 Git commit SHA；Dockerfile 会拒绝空值或其他格式，
避免生成无法追溯的镜像。

仓库不写死基础镜像 digest，因为 digest 必须由交付方在镜像仓库和目标架构上验证，
不能凭空填写。正式交付应先解析并通过漏洞扫描批准两个基础镜像，再用不可变 digest
覆盖构建参数：

```bash
docker build \
  --build-arg RUST_IMAGE=rust:1.97.1-bookworm@sha256:<approved-digest> \
  --build-arg DEBIAN_IMAGE=debian:12-slim@sha256:<approved-digest> \
  --build-arg RYFRAME_BUILD_COMMIT=<full-commit-sha> \
  --file deploy/Dockerfile .
```

稳定发布工作流只发布 GitHub 自动生成的源码快照，不构建或分发任何平台镜像。生产镜像必须由部署环境从同一 annotated tag 解引用出的精确提交独立构建、扫描并记录不可变摘要；升级 Rust 或 Debian 后必须重新运行全量测试、迁移/恢复演练和容量基准。

部署环境完成构建和扫描后，应将镜像推送到自身受控的内部仓库，并把
`repository@sha256:digest` 写入 `RYFRAME_IMAGE`。`deploy/.env.production.example` 中的值
仅表示部署方构建产物，不引用 GHCR 或任何 GitHub Release 自定义附件。

`deploy/compose.prod.yml` 仅编排 API、独立迁移进程和独立 Worker，MySQL、Redis 与对象存储均应为受控网络中的外部托管服务。复制 `deploy/.env.production.example` 到部署平台配置，生成主库密码、数据库副本 JSON、业务数据源 JSON、Redis 密码、对象存储凭据、JWT 密钥和指标 Token 文件，并全部以 Docker secret 只读挂载；没有副本或业务数据源时对应文件内容为 `[]`。不得把真实密码、令牌、私钥、连接 JSON 或可变镜像 tag 写入仓库。Compose 中 API 与 Worker 的 Snowflake 节点号必须不同，镜像必须采用部署环境构建并审计过的 digest 引用。

Compose 默认使用只读根文件系统、删除全部 Linux capability、启用 `no-new-privileges`、限制
每个容器最多 256 个进程，并给停止流程保留 15 秒优雅退出时间。CPU、内存和更严格的进程
配额必须按部署平台的容量测试结果设置，不能把示例值直接当作所有环境的资源预算。

前端不加入后端 Compose。部署平台必须从 `ryframe-vue3` 独立仓库检出与后端同名的 annotated tag，核验并记录该 tag object ID 及其解引用出的完整 40 位提交，再执行冻结锁文件安装与生产构建并把 `dist/` 作为静态站点独立发布；部署记录同时保存前后端 tag object ID 与提交 SHA，不能从可变分支或后端工作树临时复制前端产物。

生产配置与 Compose 将 `APP_LOGGER_OUTPUT` 默认设为 `stdout`，API 和 Worker 均由容器平台
采集、轮转和保留日志，不会在只读根文件系统中创建 `logs/`。只有在已挂载可写持久卷并
明确需要进程内文件滚动时才能改为 `file`；此时使用 `APP_LOGGER_RETENTION_DAYS` 设置
1–3650 个按日滚动文件的上限，并把平台侧与应用侧保留策略一并纳入容量告警。

生产的 `APP_DATABASE_SQL_LOG_LEVEL` 默认必须为 `off`，`APP_DATABASE_SQL_SLOW_THRESHOLD_MS`
默认 200。只有在记录开始、结束和负责人后的短时排障窗口，才可以改为 `slow`、`summary`
或 `full`；`summary` 与 `full` 会在启动时输出安全警告。无论模式如何，日志和 OpenTelemetry
均不得记录绑定参数、密码、令牌或连接串；OpenTelemetry 数据库 span 不记录原始 SQL。

## 2. 网络与入口

- 仅 Nginx/负载均衡器暴露 443；应用 8080、MySQL、Redis 和对象存储 API 只在受控
  网络内开放。
- 生产的 `ryframe-worker` 使用 `jobs.health_port`（默认 `9091`）提供内网
  `/livez`、`/readyz` 和 `/metrics`。该端口只允许 Prometheus/VPN 与编排探针访问，
  不得经公网 Nginx 暴露；外置 Worker 的队列告警必须抓取该端点。
- 使用 `deploy/nginx/ryframe.conf` 时，将 metrics 的示例私网 CIDR 替换为精确的
  Prometheus/VPN 地址。安全组也应执行同样限制，不能只依赖 Nginx。
- Nginx 必须覆盖客户端提供的转发头；`APP_PROXY_TRUSTED_CIDRS` 仅包含真实代理地址。
- `/api/v1/ws` 必须使用 WebSocket 专用反向代理：转发 `Upgrade`，关闭缓冲，并将读取超时设为高于心跳间隔。一次性 ticket 位于查询参数，Nginx、负载均衡器和 CDN 均不得记录完整请求 URI 或 ticket；模板已对该路径关闭访问日志。
- 生产启用消息中心时必须保持 `APP_REDIS_MODE=required`。默认容量为票据 60 秒、消息保留 90 天、每租户用户单实例 5 条连接、每连接 256 条有界出站队列和单消息最多 100000 名收件人；共享补拉调度器每 15 秒扫描一次、启动抖动最多 5 秒、每个租户用户每批最多 100 条，查询一次后向该身份的全部连接扇出。调整 `APP_MESSAGING_*` 前必须完成连接、内存、数据库写放大和大受众发布压测。
- `APP_API_DOCS_ENABLED=false`，并在 Nginx 阻断 Swagger/OpenAPI；`APP_MONITOR_METRICS_BEARER_TOKEN_FILE`
  指向独立随机 secret 文件。配置和轮换方法见[值班手册](operations-runbook.md)。

## 3. 传输加密

公网 TLS 在 Nginx 终止，最低启用 TLS 1.2/1.3、完整证书链、HSTS 和自动续期。使用
blackbox exporter 从外部持续验证域名、SNI 和证书到期时间。

远程 MySQL 必须设置 `tls_mode = "verify_identity"` 并提供 `tls_ca` 路径；需要 mTLS
时同时设置客户端证书与私钥。仅当数据库与应用位于同一受控主机或经过平台提供的
加密隧道时，才可以经过风险确认使用明文：

```text
APP_DATABASE_TLS_MODE=verify_identity
APP_DATABASE_TLS_CA=/run/secrets/mysql-ca.pem
APP_DATABASE_TLS_CLIENT_CERT=/run/secrets/mysql-client.pem
APP_DATABASE_TLS_CLIENT_KEY=/run/secrets/mysql-client.key
```

远程 Redis 必须启用证书校验的 `rediss://`，并将 CA/客户端证书作为只读 secret 挂载：

```text
APP_REDIS_TLS=true
APP_REDIS_TLS_CA=/run/secrets/redis-ca.pem
APP_REDIS_TLS_CLIENT_CERT=/run/secrets/redis-client.pem
APP_REDIS_TLS_CLIENT_KEY=/run/secrets/redis-client.key
```

MySQL/Redis 的服务地址必须与证书身份匹配。禁止通过关闭主机名或证书校验解决证书错误。
对象存储必须同时使用 `APP_OBJECT_STORAGE_USE_SSL=true` 和 `https://` endpoint；显式
`http://` endpoint 即使开启 `use_ssl` 也会被启动校验拒绝。其 CA 应进入容器系统信任链。

## 4. 对象存储和横向扩展

多实例生产默认使用 RustFS、MinIO 或 S3。`local` 仅允许：

1. 明确的单应用实例；或
2. 所有实例挂载同一具备锁、原子重命名和 read-after-write 语义的共享卷。

使用 `local` 时必须显式设置 `APP_OBJECT_STORAGE_ALLOW_LOCAL_IN_PRODUCTION=true`，
这是风险确认而不是一致性保证。容器的 `/var/lib/ryframe/uploads` 必须绑定持久卷并
纳入容量、快照、恢复和跨故障域规划；未挂载持久卷时容器重建会造成文件不可用。通用生产
镜像不声明匿名 `VOLUME`，默认 Compose 也不挂载上传目录；启用本地后端的部署必须在 API 与
Worker 服务上显式配置同一个具名卷或经过验证的共享挂载，避免 Docker 隐式创建失管卷。

滚动/蓝绿部署期间，新旧实例必须看到同一对象集合。无法证明共享文件系统语义时，
必须改用 S3 兼容后端。

## 5. 最低上线检查

- 配置校验通过，密钥均由 secret 管理注入，日志中无敏感值。
- API 与 Worker 默认向 stdout 输出 JSON 日志，平台采集、轮转、保留和容量告警均已生效。
- `/livez=200`、`/readyz=200`，公网 Swagger/OpenAPI 为 `404`。
- 登录后可建立 `/api/v1/ws` 并收到 `101 Switching Protocols`；抽查 Nginx、负载均衡器和 CDN 日志，确认其中不包含 `ticket=`。
- 并发建立超过 `APP_MESSAGING_MAX_CONNECTIONS_PER_USER` 的同租户用户连接会被拒绝；慢消费者以 `1013` 关闭，大受众发布超过配置上限时整笔事务回滚且无部分收件人记录。
- Prometheus 从允许网段携带 Bearer Token 抓取成功；公网或无 Token 请求被拒绝。
- MySQL/Redis/对象存储 TLS 证书校验成功，数据库迁移和隔离库恢复演练通过。
- API 与 Worker 使用相同的 `APP_DATA_RETENTION_*` 和 `APP_USER_IMPORT_*`；用户导入私有 bucket 对两类进程都可读写。
- API 与 Worker 使用相同的 `APP_TENANT_CONFIG_TRANSFER_*`；默认保持 5 MiB 压缩包、20 MiB
  解压、10,000 项、168 小时 artifact 和 168 小时 rollback 窗口。两类进程都能访问私有
  `config-packages` bucket，bucket policy 不允许匿名读取或客户端选择对象路径。
- 先部署迁移，再部署已经注册 `system.user.import` 和数据保留处理器的新 Worker，然后部署 API，最后发布前端；不得先让新 API 向旧 Worker 入队未知任务。
- 配置迁移同样遵循“数据库迁移 → 已注册 export/preview/apply/rollback 处理器的新 Worker → API
  → 前端”；上线前生成配置包并完成一次只读预览，不得让新 API 向旧 Worker 入队未知任务。
- 验证普通配置写在有效迁移租约期间返回 `409`，租约锁顺序为“租户行 → 租约行 → 资源/关系行”；
  预览后修改目标配置再应用旧计划必须因 `configuration_version` 或授权纪元变化拒绝。
- 在隔离租户演练应用前快照失败、应用事务回滚与 168 小时窗口内回滚。对象过期不等于配置包或
  迁移历史元数据过期；当前没有配置这些历史记录的自动硬删除期。
- 在系统租户执行数据保留预览，逐项核对截止时间和候选数量；确认默认计划只在下一个 `03:30 UTC` 执行，未因升级立即删除历史。
- 告警规则加载成功，Alertmanager 测试通知到达；备份与证书指标有实际数据。
- 容量验收通过并保留报告；滚动部署各实例 Snowflake worker ID 唯一。

完整发布顺序见[发布与回滚指南](release-guide.md)，运行时处置见
[生产监控与值班手册](operations-runbook.md)。
数据保留、异步导入、权限诊断和租户总览的上线边界见
[数据生命周期、异步导入与运维诊断](data-lifecycle.md)。
租户配置迁移的包格式、稳定键和回滚边界见[租户配置包迁移](tenant-config-transfer.md)。
