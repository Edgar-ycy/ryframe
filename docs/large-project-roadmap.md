# RyFrame 大型项目开发路线图

> 当前状态：v0.5.0 高级特性全部完成。工程化（CI/CD、测试体系、审计）待完善。
> **最后更新**：2026-05-31

---

## 📊 当前已有能力

### 架构与基础设施
- ✅ **分层架构**：13 个 crate，四层依赖模型（基础共享→基础设施→领域→接入）
- ✅ **Cargo Workspace**：统一依赖管理，resolver = "3"
- ✅ **配置管理**：四环境 TOML 配置 + 环境变量覆盖 + 配置热加载
- ✅ **数据库支持**：PostgreSQL / MySQL / SQLite，连接池管理 + 多数据源读写分离

### 认证与安全
- ✅ **JWT 认证**：access_token + refresh_token 双令牌 + Token 黑名单
- ✅ **RBAC 权限**：用户→角色→权限 多对多模型 + 数据权限
- ✅ **密码哈希**：argon2
- ✅ **认证中间件**：auth_middleware + 在线用户跟踪
- ✅ **安全防护**：XSS 过滤、安全响应头（CSP/HSTS/X-Frame）、幂等性、重放防护、CSRF

### 业务功能
- ✅ **19 张表**：15 核心 + job/job_log/role_dept
- ✅ **80+ REST API**：auth / system / monitor / tools / common 完整路由
- ✅ **代码生成器**：实体/仓库/服务/Handler 模板
- ✅ **定时任务**：Cron 调度 + 任务管理 + 执行历史
- ✅ **对象存储**：本地 + MinIO/S3

### 中间件（16 个）
- ✅ CORS / RequestId / 请求日志（含脱敏） / 限流（全局+用户+接口级）
- ✅ XSS 过滤 / 安全头 / 超时控制 / 请求体限制
- ✅ WebSocket 支持 / 响应压缩 / 幂等性 / 重放保护
- ✅ ETag 缓存控制 / Prometheus Metrics / OpenTelemetry 链路追踪
- ✅ 操作日志记录中间件 / CSRF 防护

### 高级特性
- ✅ 统一缓存 trait 抽象（Redis / Local / Noop）
- ✅ 缓存防护（防穿透/击穿/雪崩）+ 缓存预热
- ✅ 消息队列抽象（Kafka / InMemory / Noop 委托模式）
- ✅ 多租户（Header/Subdomain/PathPrefix 识别 + 数据隔离 + 配额）
- ✅ 熔断器（CircuitBreaker 三态模型）
- ✅ 分布式锁（Redis）
- ✅ 事件总线（EventBus）
- ✅ 功能开关（Feature Flags）
- ✅ gRPC 服务端/客户端
- ✅ 多数据源动态路由 + 读写分离
- ✅ 异步任务队列

### 可观测性
- ✅ OpenTelemetry 链路追踪
- ✅ Prometheus Metrics 端点（HTTP + 进程 CPU/内存/FD/线程）
- ✅ 服务器信息 / 健康检查 / 缓存统计 / DB 连接池监控
- ✅ 慢查询日志告警（SqlLogLayer + slow_query_threshold_ms）
- ✅ AlertManager 告警规则（HTTP / 进程 / 安全 / 流量共 14 条）
- ✅ Prometheus 抓取配置 + Grafana Dashboard 模板

### API 文档
- ✅ OpenAPI JSON + Swagger UI
- ✅ 国际化（zh-CN / en-US）

### 部署
- ✅ Docker 多阶段构建（rust:1.85-slim → debian:bookworm-slim，端口 3000 + HEALTHCHECK /health）
- ✅ docker-compose（MySQL 8.0 / Redis 7 / Nginx）
- ✅ K8s all-in-one 部署清单
- ✅ deploy.sh 一键部署脚本
- ✅ Nginx 反向代理配置（限流/安全头/静态缓存）
- ✅ Grafana Dashboard 模板 + Prometheus 配置

---

## 🔴 高优先级（待完成）

### 1. 代码质量门禁

- [x] `cargo check --workspace` 零错误
- [x] `cargo clippy --workspace -- -D warnings` 零 warning ✅
- [x] `cargo fmt --check --all` 格式一致性 ✅
- [x] `cargo audit` 依赖漏洞检查（配置文件已有 `.cargo/audit.toml`，CI 中自动运行）
- [x] pre-commit hooks（格式 + lint + 测试）✅

### 2. 测试体系建设

#### 2.1 Service 层单元测试
- [x] `ryframe-service/tests/service_tests.rs`（已有基础测试）
- [x] 各 service 独立测试文件 + mock 完善

#### 2.2 Repository 层单元测试
- [x] `ryframe-db/tests/user_test.rs`（已有基础测试）
- [x] 各 repo 独立测试文件

#### 2.3 API 集成测试
- [x] 认证流程测试（登录/刷新/登出/me）（`integration_test.rs` 已有）
- [x] 用户 CRUD 端到端测试
- [x] 权限校验测试

#### 2.4 基础设施
- [x] 测试数据工厂 (Test Fixtures)
- [x] 测试覆盖率报告 (cargo-tarpaulin / cargo-llvm-cov)
- [x] CI 中测试覆盖率门禁 (≥ 70%)

### 3. CI/CD 流水线

- [x] GitHub Actions 工作流文件已有（`.github/workflows/ci.yml`）
- [x] 确认 CI 实际运行：clippy + fmt 门禁 ✅
- [x] GitHub Actions: test + coverage 报告 ✅
- [x] GitHub Actions: cargo audit 安全扫描 ✅
- [x] GitHub Actions: Docker 镜像构建 ✅
- [x] 自动生成测试覆盖率 Badge ✅

### 4. 安全加固

- [x] 登录失败锁定机制（`auth_config.max_login_attempts`, `lockout_duration_minutes`）
- [x] 密码复杂度校验强化（`password::validate_complexity`, 大写+小写+数字+特殊字符）
- [x] 敏感配置加密存储方案（AES-256-GCM + CONFIG_MASTER_KEY 环境变量）
- [x] 文件上传类型白名单
- [x] CSP (Content-Security-Policy) 安全头
- [x] CSRF Token 防护（Double-Submit Cookie + HMAC-SHA256）

---

## 🟡 中优先级

### 5. 可观测性完善

- [x] Grafana Dashboard JSON 模板（Prometheus 数据源）
- [x] AlertManager 告警规则
- [x] 慢查询日志（`slow_query_threshold_ms` + SqlLogLayer WARN 告警）
- [x] 结构化日志统一为 JSON 格式（通过 logger.format 配置）
- [x] 链路追踪 span 细化（DB 查询 / 外部调用）

### 6. Kubernetes 部署

- [x] Deployment + Service + ConfigMap（`deploy/k8s/all-in-one.yaml`）
- [x] Helm Chart 封装（`deploy/helm/ryframe/`，含 values/values-prod）
- [x] liveness / readiness probe（`/health` 端点 + Helm 模板配置）
- [x] HPA 自动伸缩配置（CPU + Memory，Helm 模板化）
- [x] Ingress 配置（TLS + cert-manager，Helm 模板化）

### 7. 数据治理

- [x] 数据库备份脚本（`deploy.sh` 内置）
- [x] 操作日志自动归档/清理（CleanOperLogTask + CleanLoginInfoTask 内置定时任务）
- [x] 连接池健康检查增强
- [x] 数据库慢查询监控集成（`slow_query_threshold_ms` 配置项）

### 8. 缓存策略层

- [x] 统一缓存 trait 抽象
- [x] Redis 缓存实现
- [x] 本地内存缓存
- [x] 缓存穿透防护（空值缓存）
- [x] 缓存击穿防护（互斥锁 + 双检锁）
- [x] 缓存雪崩防护（随机 TTL 抖动）
- [x] 缓存预热机制

### 9. 开发者体验

- [x] `.editorconfig` 统一编辑器配置
- [x] `.devcontainer` 开发容器
- [x] Conventional Commits 规范文档
- [x] `CONTRIBUTING.md` 贡献指南
- [x] `CHANGELOG.md` 变更日志

---

## 🟢 低优先级（已全部完成 ✅）

### 10. 消息队列集成 ✅

- [x] 消息队列 trait 抽象（`MessageQueue`）
- [x] Kafka 适配器（`rdkafka` + `kafka` feature）
- [x] InMemory 开发实现（tokio broadcast）
- [x] MqBackend 枚举委托模式

### 11. 多租户 ✅

- [x] 租户识别中间件（`tenant_middleware`）
- [x] 三种提取方式（Header/Subdomain/PathPrefix）
- [x] 数据隔离策略（SharedTable/DatabasePerTenant/SchemaPerTenant）
- [x] TenantFilter<T> 自动过滤
- [x] 租户配额管理（`TenantQuota`）

### 12. API 网关增强 ✅

- [x] 响应缓存头（ETag / Cache-Control + 304）
- [x] API 版本协商（`versioning.rs`）
- [x] 请求/响应日志脱敏
- [x] 限流策略增强（per-user / per-api 三层限流）

### 13. 性能与压测 ✅

- [x] 压力测试脚本（`deploy/tests/stress-test.js`）
- [x] 冒烟测试脚本（`deploy/tests/smoke-test.js`）
- [x] 性能基准测试（criterion benches）
- [x] 连接池调优指南（见 `db-guide.md` §连接池调优）

---

## 📈 度量指标

| 指标 | 当前 | 目标 |
|------|------|------|
| 测试覆盖率 | ~60% | ≥ 70% |
| Clippy Warning | 0 ✅ | 0 real warnings |
| CI 自动化 | CI 文件已有 ✅ | 100% 流水线通过 |
| API 集成测试 | 部分 | 全覆盖核心流程 |
| 安全扫描 | 配置文件已有 | cargo audit 零严重漏洞 |
| K8s 部署 | Helm Chart ✅ | 完整 Helm Chart |

---

> **下一步**：数据治理提升、集成测试增强。清理已完成的待办项。
