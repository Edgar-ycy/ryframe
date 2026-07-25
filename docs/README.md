# RyFrame 文档索引

本目录存放后端、数据库、架构、部署运维和前端集成相关文档。

- [前端集成指南](frontend-integration.md)：给 `ryframe-vue3` 使用的接口响应、认证、动态菜单、分页、上传下载和监控接口约定。
- [API 使用指南](api-guide.md)：后端接口、认证和业务模块说明。
- [架构与演进指南](architecture.md)：当前 crate 依赖、请求链路、架构问题、目标边界和分阶段改造计划。
- [数据库指南](db-guide.md)：主库/只读副本、命名业务数据源、实体、仓储、迁移和 SQL 初始化说明。
- [对象存储与 RustFS 指南](storage-guide.md)：RustFS 本机启动、配置、运行时检查、上传下载和 CI 覆盖。
- [v0.5 发布与回滚指南](release-guide.md)：RC 观察、联合标签、源码 Release、蓝绿切换和回滚约束。
- [生产部署基线](production-deployment.md)：镜像、网络、TLS、metrics、API 文档和多实例存储约束。
- [生产监控与值班手册](operations-runbook.md)：Prometheus 采集、告警、分级响应和故障处置。
- [容量测试与验收标准](capacity-guide.md)：负载模型、测试场景、SLO 门槛和容量报告证据。
- [依赖维护与重复版本治理](dependency-maintenance.md)：安全审计、重复版本基线和升级规则。
