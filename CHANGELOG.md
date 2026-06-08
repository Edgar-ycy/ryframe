# Changelog

所有值得注意的项目变更都将记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

---

## [v0.1.0] - 2026-06-08

### 🎉 首个稳定版本

基于 Rust + Axum 的现代化企业级后端框架，提供开箱即用的认证授权、系统管理、监控运维等完整能力。

### ✨ 新增

- **认证授权**：JWT 登录/刷新/黑名单 + RBAC 权限模型 + 数据权限 DataScope
- **系统管理**：用户/角色/权限/菜单/部门/岗位/参数/字典/通知 完整 CRUD
- **安全防护**：XSS 过滤、多层限流（全局/用户级/接口级）、防重放攻击、幂等性、安全响应头
- **缓存体系**：Redis 缓存（配置/字典/菜单树/部门树），读缓存+写失效+缓存击穿保护，无 Redis 时自动降级内存模式
- **监控运维**：服务器信息、增强健康检查（DB+Redis 连通性）、DB 连接池、在线用户、缓存统计、Prometheus Metrics
- **链路追踪**：OpenTelemetry 分布式追踪（可配置采样率）
- **定时任务**：Cron 调度 + 任务管理 + 执行历史 + 内置清理任务
- **代码生成器**：读取表结构自动生成 Entity/Repository/Service/Handler/DTO 五层 CRUD 代码
- **弹性容错**：重试（指数退避）+ 熔断器 + 降级
- **数据访问**：MySQL/PostgreSQL/SQLite 三数据库支持，多数据源动态切换 + 读写分离
- **配置热加载**：运行时自动检测并应用配置变更
- **消息队列**：Kafka 集成 + 进程内内存队列
- **分布式锁**：Redis 分布式锁
- **事件总线**：进程内异步事件发布/订阅
- **多租户**：租户隔离（数据库级/Schema 级）+ 租户配额
- **gRPC**：Tonic 服务端/客户端
- **WebSocket**：WebSocket 连接管理与消息广播
- **对象存储**：本地 / MinIO / S3 多后端动态切换
- **文件处理**：文件上传/下载 + 图片压缩 + Excel 导入导出
- **国际化**：i18n 多语言支持（中文/英文）
- **Swagger UI**：交互式 API 文档

### 🏗 架构

- Cargo Workspace 分层架构（12 个 crate）
- 五层分层模型：基础共享 → 基础设施 → 领域 → 接入 → 入口
- 面向 trait 编程，Service/Repository 可 Mock 测试
- 构造函数注入 + AppState 集中管理依赖

### 🛠 CI/CD

- GitHub Actions CI（fmt / clippy / test / coverage / security-audit）
- 代码覆盖率上传 Codecov（门槛 60%）
- Nightly 自动发布（push main）
- Stable 版本通过 `v*` tag 手动触发发布
