# RyFrame 文档索引

本目录存放后端、数据库、架构、部署运维和前端集成相关文档。

- [前端集成指南](frontend-integration.md)：给 `ryframe-vue3` 使用的接口响应、认证、动态菜单、分页、上传下载和监控接口约定。
- [API 使用指南](api-guide.md)：后端接口、认证和业务模块说明。
- [架构与演进指南](architecture.md)：当前 crate 依赖、请求链路、架构问题、目标边界和分阶段改造计划。
- [数据库指南](db-guide.md)：主库/只读副本、命名业务数据源、实体、仓储、迁移和 SQL 初始化说明。
- [缓存命名空间一致性协议](cache-namespace.md)：数据库权威版本、事务性 Outbox、Redis 固定键与强一致回源约束。
- [对象存储与 RustFS 指南](storage-guide.md)：RustFS 本机启动、配置、运行时检查、上传下载和静态门禁。
- [稳定发布与回滚指南](release-guide.md)：稳定版同名 annotated tag、精确提交身份校验、纯源码 Release、蓝绿切换和回滚约束。
- [生产部署基线](production-deployment.md)：镜像、网络、TLS、metrics、API 文档和多实例存储约束。
- [生产监控与值班手册](operations-runbook.md)：Prometheus 采集、告警、分级响应和故障处置。
- [定时任务使用与维护](job-scheduling.md)：可视化规则生成、最近执行时间预览、运行开关和后期移除步骤。
- [容量测试与验收标准](capacity-guide.md)：负载模型、测试场景、SLO 门槛和容量报告证据。
- [依赖维护与重复版本治理](dependency-maintenance.md)：安全审计、重复版本基线和升级规则。
- [FILE-A 文件摘要与上传预留单向维护](file-maintenance.md)：SHA-256 回填、旧预留清理与最终迁移前置条件。
